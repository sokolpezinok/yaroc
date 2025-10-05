#[cfg(feature = "nrf")]
use embassy_nrf::uarte::UarteRxWithIdle;

use crate::error::Error;
use crate::punch::{LEN, RawPunch, SiPunch};

/// A trait for reading from a UART until it's idle.
pub trait RxWithIdle {
    /// Read from UART until it's idle. Return the number of read bytes.
    fn read_until_idle(
        &mut self,
        buf: &mut [u8],
    ) -> impl core::future::Future<Output = crate::Result<usize>>;
}

/// Implementation of `RxWithIdle` for `UarteRxWithIdle`.
#[cfg(feature = "nrf")]
impl RxWithIdle for UarteRxWithIdle<'static> {
    /// Reads from the UART until it's idle.
    ///
    /// This is a wrapper around `embassy_nrf::uarte::UarteRxWithIdle::read_until_idle` that
    /// maps the error type.
    async fn read_until_idle(&mut self, buf: &mut [u8]) -> crate::Result<usize> {
        self.read_until_idle(buf).await.map_err(|_| Error::UartReadError)
    }
}

/// SportIdent UART reader.
///
/// This struct reads data from a UART, finds SI punches in the data stream, and returns them.
/// It is generic over the UART reader, so it can be used with different UART implementations.
pub struct SiUart<R: RxWithIdle> {
    rx: R,
    buf: [u8; LEN * 5],
    end: usize,
}

impl<R: RxWithIdle> SiUart<R> {
    /// Creates a new `SiUart` from a UART reader.
    pub fn new(rx: R) -> Self {
        Self {
            rx,
            buf: [0; LEN * 5],
            end: 0,
        }
    }

    /// Reads a single SI punch from the UART.
    ///
    /// This function reads from the UART until a punch is found. It handles cases where the
    /// punch is split across multiple reads.
    ///
    /// Returns a `RawPunch` if a punch is successfully read, or an error if reading from the
    /// UART fails or if the data cannot be parsed.
    pub async fn read(&mut self) -> crate::Result<RawPunch> {
        let bytes_read = self
            .rx
            .read_until_idle(&mut self.buf[self.end..])
            .await
            .map_err(|_| Error::UartReadError)?;
        self.end += bytes_read;

        let Some((raw, rest)) = SiPunch::find_punch_data(&self.buf[..self.end]) else {
            // Clean the buffer if we can't find punches
            if self.end >= LEN * 2 {
                self.buf.copy_within(LEN * 2..self.end, 0);
                self.end -= LEN * 2;
            }
            return Err(Error::UartReadError);
        };
        let range = self.end - rest.len()..self.end;
        self.end = range.len();
        self.buf.copy_within(range, 0);

        Ok(raw)
    }
}

#[cfg(test)]
mod test {
    use chrono::DateTime;
    use embassy_futures::block_on;
    use embassy_sync::pipe::{Pipe, Reader};

    use crate::RawMutex;

    use super::*;

    const FAKE_CAPACITY: usize = LEN * 10;

    impl RxWithIdle for Reader<'_, RawMutex, FAKE_CAPACITY> {
        async fn read_until_idle(&mut self, buf: &mut [u8]) -> crate::Result<usize> {
            Ok(self.read(buf).await)
        }
    }

    #[test]
    fn test_correct_punches() {
        let time1 = DateTime::parse_from_rfc3339("2023-11-23T10:00:03.792968750+01:00").unwrap();
        let punch1 = SiPunch::new(46283, 47, time1, 1);

        let mut pipe: Pipe<RawMutex, FAKE_CAPACITY> = Pipe::new();
        let (pipe_rx, pipe_tx) = pipe.split();
        block_on(pipe_tx.write(b"\x03"));
        block_on(pipe_tx.write(&punch1.raw));

        let time2 = DateTime::parse_from_rfc3339("2023-11-23T10:02:43.792968750+01:00").unwrap();
        let punch2 = SiPunch::new(46289, 94, time2, 1);
        block_on(pipe_tx.write(&punch2.raw[1..]));

        let mut si_uart = SiUart::new(pipe_rx);
        assert_eq!(block_on(si_uart.read()).unwrap(), punch1.raw);

        // Now inject punch1 again but in two parts
        block_on(pipe_tx.write(b"\xff\x02"));
        assert_eq!(block_on(si_uart.read()).unwrap(), punch2.raw);
        block_on(pipe_tx.write(&punch1.raw[2..]));
        assert_eq!(block_on(si_uart.read()).unwrap(), punch1.raw);
    }

    #[test]
    fn test_zeroed_bytes_first() {
        let mut pipe: Pipe<RawMutex, FAKE_CAPACITY> = Pipe::new();
        let (pipe_rx, pipe_tx) = pipe.split();
        let mut si_uart = SiUart::new(pipe_rx);

        // We send 100 bytes which are empty: no punches, no headers.
        block_on(pipe_tx.write(&[0; 100]));
        assert!(block_on(si_uart.read()).is_err());

        // Then finally we send a punch.
        let time1 = DateTime::parse_from_rfc3339("2023-11-23T10:00:03.792968750+01:00").unwrap();
        let punch1 = SiPunch::new(46283, 47, time1, 1);
        block_on(pipe_tx.write(&punch1.raw));
        assert_eq!(block_on(si_uart.read()).unwrap(), punch1.raw);
    }
}
