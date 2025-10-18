//! A module for UART communication with SportIdent devices.
//!
//! This module provides a `SiUart` struct that can read SportIdent punches from a UART
//! interface. It is designed to be generic over the UART reader, so it can be used with
//! different UART implementations, like `embassy-nrf::uarte` or a custom reader for testing.
//!
//! The `SiUart` struct reads data from the UART, finds SI punches in the data stream, and
//! returns them as `BatchedPunches`. It can handle cases where punches are split across
//! multiple reads and can filter out garbage data.
//!
//! # Example
//!
//! ```no_run
//! use embassy_nrf::uarte::{UarteRxWithIdle, Config};
//! use yaroc_common::si_uart::{SiUart, RxWithIdle};
//!
//! // Initialize your UART peripheral here
//! let mut uarte = ...;
//! let mut si_uart = SiUart::new(uarte);
//!
//! loop {
//!     match si_uart.read().await {
//!         Ok(punches) => {
//!             for punch in punches.iter() {
//!                 // Process the punch
//!             }
//!         }
//!         Err(e) => {
//!             // Handle the error
//!         }
//!     }
//! }
//! ```

#[cfg(feature = "nrf")]
use embassy_nrf::uarte::UarteRxWithIdle;

use crate::backoff::BatchedPunches;
use crate::error::Error;
use crate::punch::{LEN, SiPunch};

/// A trait for reading from a UART that can detect when the line is idle.
pub trait RxWithIdle {
    /// Read from the UART until the line is idle.
    ///
    /// This function should read from the UART and store the data in `buf`. It should return
    /// when the UART line has been idle for a certain amount of time.
    ///
    /// # Arguments
    ///
    /// * `buf` - The buffer to read the data into.
    ///
    /// # Returns
    ///
    /// A `Result` containing the number of bytes read, or an error if reading fails.
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

const PUNCH_CAPACITY: usize = 12;
const BUF_SIZE: usize = LEN * PUNCH_CAPACITY;

/// A SportIdent UART reader.
///
/// This struct reads data from a UART, finds SI punches in the data stream, and returns them.
/// It is generic over the UART reader, so it can be used with different UART implementations.
///
/// # Type Parameters
///
/// * `R` - A UART reader that implements the `RxWithIdle` trait.
pub struct SiUart<R: RxWithIdle + Send> {
    rx: R,
    buf: [u8; BUF_SIZE],
    end: usize,
}

impl<R: RxWithIdle + Send> SiUart<R> {
    /// Creates a new `SiUart` from a UART reader.
    ///
    /// # Arguments
    ///
    /// * `rx` - A UART reader that implements the `RxWithIdle` trait. The reader should be
    ///   configured with the correct baud rate for the SportIdent device (usually
    ///   38400 bps).
    pub fn new(rx: R) -> Self {
        Self {
            rx,
            buf: [0; LEN * 12],
            end: 0,
        }
    }

    /// Reads SI punches from the UART.
    ///
    /// This function reads data from the UART, searches for SI punches, and returns them.
    /// It waits for the UART line to be idle before processing the received data. This function
    /// can handle cases where punches are split across multiple reads and can filter out
    /// garbage data.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `BatchedPunches` struct, which is a collection of raw punch
    /// data. If no punches are found, the `BatchedPunches` struct will be empty.
    ///
    /// # Errors
    ///
    /// This function can return the following errors:
    ///
    /// * `Error::UartReadError` - If there is an error reading from the UART.
    /// * `Error::UartClosedError` - If the UART is closed.
    pub async fn read(&mut self) -> crate::Result<BatchedPunches> {
        let bytes_read = self.rx.read_until_idle(&mut self.buf[self.end..]).await?;
        self.end += bytes_read;

        if bytes_read == 0 {
            return Err(Error::UartClosedError);
        }

        let mut punches = BatchedPunches::new();
        let mut payload = &self.buf[..self.end];
        while let Some((punch, rest)) = SiPunch::find_punch_data(payload) {
            if punches.len() < punches.capacity() {
                punches.push(punch).unwrap();
                payload = rest;
            } else {
                break;
            }
        }

        if punches.is_empty() && self.end >= 2 * LEN {
            // Clean the beginning of the buffer if we can't find punches
            let range = LEN..self.end;
            self.buf.copy_within(range, 0);
            self.end -= LEN;
            return Err(Error::UartReadError);
        }

        let range = self.end - payload.len()..self.end;
        self.end = range.len();
        self.buf.copy_within(range, 0);

        Ok(punches)
    }
}

#[cfg(test)]
mod test {
    use chrono::DateTime;
    use embassy_futures::block_on;
    use embassy_sync::pipe::{Pipe, Reader};

    use crate::RawMutex;

    use super::*;

    const FAKE_CAPACITY: usize = LEN * 12;

    impl RxWithIdle for Reader<'_, RawMutex, FAKE_CAPACITY> {
        async fn read_until_idle(&mut self, buf: &mut [u8]) -> crate::Result<usize> {
            Ok(self.read(buf).await)
        }
    }

    #[test]
    fn test_correct_punches() {
        let time1 = DateTime::parse_from_rfc3339("2023-11-23T10:00:03.792968750+01:00").unwrap();
        let punch1 = SiPunch::new_send_last_record(46283, 47, time1, 1);

        let mut pipe: Pipe<RawMutex, FAKE_CAPACITY> = Pipe::new();
        let (pipe_rx, pipe_tx) = pipe.split();
        block_on(pipe_tx.write(b"\x03"));
        block_on(pipe_tx.write(&punch1.raw));

        let time2 = DateTime::parse_from_rfc3339("2023-11-23T10:02:43.792968750+01:00").unwrap();
        let punch2 = SiPunch::new_send_last_record(46289, 94, time2, 1);
        block_on(pipe_tx.write(&punch2.raw[1..]));

        let mut si_uart = SiUart::new(pipe_rx);
        let punches1 = block_on(si_uart.read()).unwrap();
        assert_eq!(punches1.as_slice(), &[punch1.raw, punch2.raw]);

        // Now inject punch1 again but in two parts
        block_on(pipe_tx.write(b"\xff\x02"));
        let punches2 = block_on(si_uart.read()).unwrap();
        assert!(punches2.is_empty());
        block_on(pipe_tx.write(&punch1.raw[2..]));
        let punches3 = block_on(si_uart.read()).unwrap();
        assert_eq!(punches3.as_slice(), &[punch1.raw]);
    }

    #[test]
    fn test_zeroed_bytes_first() {
        let mut pipe: Pipe<RawMutex, FAKE_CAPACITY> = Pipe::new();
        let (pipe_rx, pipe_tx) = pipe.split();
        let mut si_uart = SiUart::new(pipe_rx);

        // We send 38 bytes which are empty: no punches, no headers.
        block_on(pipe_tx.write(&[0; 38]));
        assert!(block_on(si_uart.read()).unwrap().is_empty());

        // Then finally we send a punch, but it's split into two parts.
        let time1 = DateTime::parse_from_rfc3339("2023-11-23T10:00:03.792968750+01:00").unwrap();
        let punch1 = SiPunch::new_send_last_record(46283, 47, time1, 1);
        block_on(pipe_tx.write(&punch1.raw[0..2]));
        assert!(block_on(si_uart.read()).is_err());
        block_on(pipe_tx.write(&punch1.raw[2..]));
        let punches = block_on(si_uart.read()).unwrap();
        assert_eq!(punches[0], punch1.raw);
    }

    #[test]
    fn test_garbage() {
        let mut pipe: Pipe<RawMutex, FAKE_CAPACITY> = Pipe::new();
        let (pipe_rx, pipe_tx) = pipe.split();
        let mut si_uart = SiUart::new(pipe_rx);

        // Fill the buffer with garbage
        block_on(pipe_tx.write(&[0xff; super::BUF_SIZE]));
        assert!(block_on(si_uart.read()).is_err());
        // The buffer should be cleaned by `LEN`
        assert_eq!(si_uart.end, super::BUF_SIZE - LEN);
    }
}
