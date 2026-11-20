//! Low-level USB serial framing and timeout handling for Postcard-encoded responses.

use std::io::Read;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use log::error;
use postcard::take_from_bytes;
use yaroc_common::usb::UsbResponse;

/// Abstraction over serial streams whose read timeout can be adjusted at runtime.
pub trait SetTimeout {
    /// Sets the read/write timeout for subsequent I/O operations on the stream.
    fn set_timeout(&mut self, timeout: Duration) -> Result<(), String>;
}

impl<T: serialport::SerialPort> SetTimeout for T {
    fn set_timeout(&mut self, timeout: Duration) -> Result<(), String> {
        serialport::SerialPort::set_timeout(self, timeout).map_err(|e| e.to_string())
    }
}

/// Buffered streaming decoder for Postcard-serialized frames read from a USB serial stream.
///
/// Handles arbitrary read fragmentation and fused frames by accumulating incoming bytes
/// in an internal buffer until complete frames can be deserialized.
pub struct PostcardReader<S> {
    stream: S,
    buf: BytesMut,
}

impl<S> PostcardReader<S> {
    /// Creates a new `PostcardReader` wrapping the given stream.
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            buf: BytesMut::with_capacity(1024),
        }
    }

    /// Returns a shared reference to the underlying stream.
    #[cfg(test)]
    pub fn get_ref(&self) -> &S {
        &self.stream
    }

    /// Returns a mutable reference to the underlying stream.
    pub fn get_mut(&mut self) -> &mut S {
        &mut self.stream
    }
}

impl<S: Read> PostcardReader<S> {
    /// Reads and deserializes the next Postcard frame from the stream.
    ///
    /// Returns `Ok(None)` if EOF is reached cleanly with no buffered trailing bytes,
    /// or `Err` if an I/O error occurs or EOF is reached mid-frame.
    fn read_one<T: serde::de::DeserializeOwned>(&mut self) -> Result<Option<T>, String> {
        let mut temp_buf = [0u8; 1024];
        loop {
            if let Some(item) = self.parse_frame() {
                return Ok(Some(item));
            }

            let n = self
                .stream
                .read(&mut temp_buf)
                .map_err(|e| format!("Reading from USB serial failed: {e}"))?;
            if n == 0 {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                return Err("Serial port closed with incomplete response".to_owned());
            }
            self.buf.extend_from_slice(&temp_buf[..n]);
        }
    }

    /// Attempts to deserialize a single frame from the internal buffer.
    ///
    /// Advances past corrupted leading bytes on deserialization errors, or returns `None`
    /// when more bytes are needed to complete the frame.
    fn parse_frame<T: serde::de::DeserializeOwned>(&mut self) -> Option<T> {
        loop {
            if self.buf.is_empty() {
                return None;
            }
            match take_from_bytes::<T>(&self.buf) {
                Ok((item, rest)) => {
                    let consumed = self.buf.len() - rest.len();
                    self.buf.advance(consumed);
                    return Some(item);
                }
                Err(postcard::Error::DeserializeUnexpectedEnd) => {
                    return None;
                }
                Err(e) => {
                    error!("Failed to parse response: {e}");
                    self.buf.advance(1);
                }
            }
        }
    }
}

impl<S: SetTimeout> PostcardReader<S> {
    /// Updates the underlying stream timeout.
    fn set_timeout(&mut self, timeout: Duration) -> Result<(), String> {
        self.stream.set_timeout(timeout)
    }
}

impl<S: Read + SetTimeout> PostcardReader<S> {
    /// Reads the next [`UsbResponse`] frame, automatically extending the stream timeout
    /// whenever a [`UsbResponse::PartialOk`] keep-alive is received.
    pub fn read_next_response(&mut self) -> Result<Option<UsbResponse>, String> {
        loop {
            match self.read_one::<UsbResponse>()? {
                Some(UsbResponse::PartialOk(timeout_ms)) => {
                    self.set_timeout(Duration::from_millis(timeout_ms as u64))?;
                }
                other => return Ok(other),
            }
        }
    }
}

/// Iterates over streamed [`UsbResponse`] frames until [`UsbResponse::Ok`] or EOF is reached.
impl<S: Read + SetTimeout> Iterator for PostcardReader<S> {
    type Item = Result<UsbResponse, String>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.read_next_response() {
            Ok(Some(UsbResponse::Ok) | None) => None,
            Ok(Some(response)) => Some(Ok(response)),
            Err(e) => Some(Err(e)),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_utils {
    use std::io::{Read, Write};
    use std::time::Duration;

    use super::SetTimeout;

    pub struct MockSerialStream {
        pub read_data: Vec<u8>,
        read_offset: usize,
        pub chunk_size: usize,
        pub written: Vec<u8>,
        pub timeouts: Vec<Duration>,
    }

    impl MockSerialStream {
        pub fn new(read_data: Vec<u8>) -> Self {
            Self {
                read_data,
                read_offset: 0,
                chunk_size: 1024,
                written: Vec::new(),
                timeouts: Vec::new(),
            }
        }

        pub fn with_chunk_size(mut self, chunk_size: usize) -> Self {
            self.chunk_size = chunk_size;
            self
        }
    }

    impl Read for MockSerialStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.read_offset >= self.read_data.len() {
                return Ok(0);
            }
            let to_read =
                self.chunk_size.min(self.read_data.len() - self.read_offset).min(buf.len());
            buf[..to_read]
                .copy_from_slice(&self.read_data[self.read_offset..self.read_offset + to_read]);
            self.read_offset += to_read;
            Ok(to_read)
        }
    }

    impl Write for MockSerialStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl SetTimeout for MockSerialStream {
        fn set_timeout(&mut self, timeout: Duration) -> Result<(), String> {
            self.timeouts.push(timeout);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use postcard::to_stdvec;

    use super::test_utils::MockSerialStream;
    use super::*;

    #[test]
    fn test_postcard_reader_fragmented_and_fused() {
        let resp1 = UsbResponse::MiniCallHomeLog(heapless::Vec::from_slice(&[1, 2, 3, 4]).unwrap());
        let resp2 = UsbResponse::MiniCallHomeLog(heapless::Vec::from_slice(&[5, 6, 7, 8]).unwrap());
        let resp_ok = UsbResponse::Ok;

        let mut stream_bytes = Vec::new();
        stream_bytes.extend_from_slice(&to_stdvec(&resp1).unwrap());
        stream_bytes.extend_from_slice(&to_stdvec(&resp2).unwrap());
        stream_bytes.extend_from_slice(&to_stdvec(&resp_ok).unwrap());

        // Fragmented reads (1 byte per read)
        let frag_stream = MockSerialStream::new(stream_bytes.clone()).with_chunk_size(1);
        let res: Vec<_> = PostcardReader::new(frag_stream).collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(res.len(), 2);
        assert_eq!(res[0], resp1);
        assert_eq!(res[1], resp2);

        // Fused reads (all bytes in 1 read) paired with incremental iteration
        let fused_stream = MockSerialStream::new(stream_bytes);
        let mut iter = PostcardReader::new(fused_stream);
        assert_eq!(iter.next(), Some(Ok(resp1)));
        assert_eq!(iter.next(), Some(Ok(resp2)));
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn test_postcard_reader_partial_ok_updates_timeout() {
        let resp_partial = UsbResponse::PartialOk(150_000);
        let resp_ok = UsbResponse::Ok;

        let mut stream_bytes = Vec::new();
        stream_bytes.extend_from_slice(&to_stdvec(&resp_partial).unwrap());
        stream_bytes.extend_from_slice(&to_stdvec(&resp_ok).unwrap());

        let stream = MockSerialStream::new(stream_bytes);
        let mut reader = PostcardReader::new(stream);
        let res = reader.read_next_response().unwrap();
        assert_eq!(res, Some(UsbResponse::Ok));
        assert_eq!(reader.get_ref().timeouts, vec![Duration::from_millis(150_000)]);
    }

    #[test]
    fn test_postcard_reader_eof_clean_and_incomplete() {
        let empty_stream = MockSerialStream::new(Vec::new());
        let mut reader = PostcardReader::new(empty_stream);
        assert_eq!(reader.read_next_response().unwrap(), None);

        // Truncated Handshake frame
        let full = to_stdvec(&UsbResponse::Handshake(
            heapless::String::try_from("YAROC").unwrap(),
            1,
        ))
        .unwrap();
        let truncated_stream = MockSerialStream::new(full[..full.len() - 2].to_vec());
        let mut reader = PostcardReader::new(truncated_stream);
        let err = reader.read_next_response().unwrap_err();
        assert!(err.contains("incomplete response"));
    }

    #[test]
    fn test_postcard_reader_skips_corrupted_leading_bytes() {
        let valid_resp = UsbResponse::Handshake(heapless::String::try_from("YAROC").unwrap(), 1);
        let mut stream_bytes = vec![0x7F, 0x7E]; // invalid enum discriminant bytes for UsbResponse
        stream_bytes.extend_from_slice(&to_stdvec(&valid_resp).unwrap());

        let stream = MockSerialStream::new(stream_bytes);
        let mut reader = PostcardReader::new(stream);
        assert_eq!(reader.read_next_response().unwrap(), Some(valid_resp));
    }
}
