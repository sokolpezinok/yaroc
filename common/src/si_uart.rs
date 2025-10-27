//! A module for UART communication with SportIdent devices.
//!
//! This module provides a `SiUart` struct that can read SportIdent punches from a UART
//! interface. It is designed to be generic over the UART reader, so it can be used with
//! different UART implementations, like `embassy-nrf::uarte` or a custom reader for testing.
//!
//! The `SiUart` struct reads data from the UART, finds SI punches in the data stream, and
//! returns them as `BatchedPunches`. It can handle cases where punches are split across
//! multiple reads and can filter out garbage data.

#[cfg(feature = "defmt")]
use defmt::{debug, error};
#[cfg(feature = "nrf")]
use embassy_nrf::uarte::UarteRxWithIdle;
use embassy_time::{Duration, Instant, WithTimeout};
use heapless::Vec;
use heapless::index_map::{Entry, FnvIndexMap};
#[cfg(not(feature = "defmt"))]
use log::{debug, error};

use crate::backoff::BatchedPunches;
use crate::error::Error;
use crate::punch::{LEN, RawPunch, SiPunch};

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

const PUNCH_CAPACITY: usize = 5;
const BUF_SIZE: usize = LEN * PUNCH_CAPACITY;

struct UnfinishedSequence {
    punches: BatchedPunches,
    deadline: Instant,
}

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
    unfinished_sequences: FnvIndexMap<u32, UnfinishedSequence, 8>,
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
            buf: [0; BUF_SIZE],
            end: 0,
            unfinished_sequences: Default::default(),
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
    pub async fn read(&mut self) -> crate::Result<Vec<RawPunch, PUNCH_CAPACITY>> {
        let bytes_read = self.rx.read_until_idle(&mut self.buf[self.end..]).await?;
        self.end += bytes_read;
        debug!("Read {} bytes from UART", bytes_read);

        if bytes_read == 0 {
            return Err(Error::UartClosedError);
        }

        let mut punches = Vec::new();
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

    /// Reads SI punches from the UART and groups them by card number.
    ///
    /// This function reads data from the UART, searches for SI punches, and groups them into
    /// `BatchedPunches` based on the card number. It is useful when punches from multiple cards
    /// are interleaved.
    ///
    /// The function waits for the UART line to be idle before processing the received data. It can
    /// handle cases where punches are split across multiple reads and can filter out garbage data.
    ///
    /// If a sequence of punches for a card is not complete within a certain timeout, it will be
    /// returned as is.
    ///
    /// # Returns
    ///
    /// A `Result` containing a vector of `BatchedPunches`, where each `BatchedPunches` contains
    /// punches from a single card.
    ///
    /// # Errors
    ///
    /// This function can return the following errors:
    ///
    /// * `Error::UartReadError` - If there is an error reading from the UART.
    /// * `Error::UartClosedError` - If the UART is closed.
    pub async fn read_grouped_punches(
        &mut self,
    ) -> crate::Result<Vec<BatchedPunches, PUNCH_CAPACITY>> {
        loop {
            let punches = match self.next_deadline() {
                Some((deadline, card)) => match self.read().with_deadline(deadline).await {
                    Ok(punches) => punches?,
                    Err(_) => match self.unfinished_sequences.entry(card) {
                        // Card `card` timed out waiting for the last record, return what we have.
                        Entry::Occupied(seq) => return Ok([seq.remove().punches].into()),
                        Entry::Vacant(_) => continue,
                    },
                },
                None => self.read().await?,
            };

            let grouped_punches: Vec<BatchedPunches, PUNCH_CAPACITY> =
                punches.into_iter().filter_map(|p| self.group_punches(p)).collect();
            if !grouped_punches.is_empty() {
                return Ok(grouped_punches);
            }
        }
    }

    /// Groups a single punch into a sequence of punches for the same card.
    ///
    /// This function takes a single `RawPunch` and adds it to a sequence of punches for the
    /// corresponding card. If the punch is a single punch (not part of a sequence), it is
    /// returned immediately. Otherwise, it is added to an internal buffer of unfinished
    /// sequences.
    ///
    /// When a sequence is complete (i.e., the last punch of a sequence is received), the complete
    /// sequence is returned.
    ///
    /// # Arguments
    ///
    /// * `punch` - The raw punch data.
    ///
    /// # Returns
    ///
    /// An `Option` containing `BatchedPunches` if a sequence is complete, otherwise `None`.
    fn group_punches(&mut self, punch: RawPunch) -> Option<BatchedPunches> {
        let card = SiPunch::bytes_to_card(&punch);
        let (idx, cnt) = SiPunch::bytes_to_idx_and_cnt(&punch);
        if idx == 0 && cnt == 1 {
            return Some([punch].into());
        }

        match self.unfinished_sequences.entry(card) {
            Entry::Occupied(mut entry) => {
                let seq = entry.get_mut();
                seq.punches.push(punch).unwrap();
                seq.deadline = Instant::now() + Duration::from_millis(300);
                if idx + 1 == cnt || seq.punches.is_full() {
                    Some(entry.remove().punches)
                } else {
                    None
                }
            }
            Entry::Vacant(punches) => {
                let seq = UnfinishedSequence {
                    punches: [punch].into(),
                    deadline: Instant::now() + Duration::from_millis(300),
                };
                if punches.insert(seq).is_err() {
                    error!("Error inserting punch from {}, seq {}/{}", card, idx, cnt);
                }
                None
            }
        }
    }

    /// Returns the deadline of the next unfinished sequence to expire.
    ///
    /// This function is used to determine the timeout for the `read_grouped_punches` function.
    /// It returns the deadline of the sequence that will expire first, along with the corresponding
    /// card number.
    ///
    /// # Returns
    ///
    /// An `Option` containing a tuple of the deadline and the card number, or `None` if there are
    /// no unfinished sequences.
    pub fn next_deadline(&self) -> Option<(Instant, u32)> {
        self.unfinished_sequences.iter().map(|(card, seq)| (seq.deadline, *card)).min()
    }
}

#[cfg(feature = "std")]
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

    #[test]
    fn test_read_grouped_punches_single() {
        let mut pipe: Pipe<RawMutex, FAKE_CAPACITY> = Pipe::new();
        let (pipe_rx, pipe_tx) = pipe.split();
        let mut si_uart = SiUart::new(pipe_rx);

        let time1 = DateTime::parse_from_rfc3339("2023-11-23T10:00:03.792968750+01:00").unwrap();
        let punch1 = SiPunch::new_send_last_record(46283, 47, time1, 1);
        block_on(pipe_tx.write(&punch1.raw));

        let punches1 = block_on(si_uart.read_grouped_punches()).unwrap();
        assert_eq!(punches1.len(), 1);
        assert_eq!(punches1[0].as_slice(), &[punch1.raw]);

        let time2 = DateTime::parse_from_rfc3339("2023-11-23T10:02:43.792968750+01:00").unwrap();
        let punch2 = SiPunch::new_send_last_record(46289, 94, time2, 1);
        block_on(pipe_tx.write(&punch2.raw));

        let punches2 = block_on(si_uart.read_grouped_punches()).unwrap();
        assert_eq!(punches2.len(), 1);
        assert_eq!(punches2[0].as_slice(), &[punch2.raw]);
    }

    #[test]
    fn test_read_grouped_punches_interleaved() {
        let mut pipe: Pipe<RawMutex, FAKE_CAPACITY> = Pipe::new();
        let (pipe_rx, pipe_tx) = pipe.split();
        let mut si_uart = SiUart::new(pipe_rx);

        let card_a = 12345;
        let time_a1 = DateTime::parse_from_rfc3339("2023-11-23T10:00:03.792968750+01:00").unwrap();
        let punch_a1 = SiPunch::new(card_a, 31, time_a1, 0, 0, 2);
        let time_a2 = DateTime::parse_from_rfc3339("2023-11-23T10:00:04.792968750+01:00").unwrap();
        let punch_a2 = SiPunch::new(card_a, 32, time_a2, 0, 1, 2);

        let card_b = 54321;
        let time_b1 = DateTime::parse_from_rfc3339("2023-11-23T10:01:00.792968750+01:00").unwrap();
        let punch_b1 = SiPunch::new_send_last_record(card_b, 47, time_b1, 1);

        block_on(pipe_tx.write(&punch_a1.raw));
        block_on(pipe_tx.write(&punch_b1.raw));

        let punches1 = block_on(si_uart.read_grouped_punches()).unwrap();
        assert_eq!(punches1.len(), 1);
        assert_eq!(punches1[0].as_slice(), &[punch_b1.raw]);

        block_on(pipe_tx.write(&punch_a2.raw));
        let punches2 = block_on(si_uart.read_grouped_punches()).unwrap();
        assert_eq!(punches2.len(), 1);
        assert_eq!(punches2[0].as_slice(), &[punch_a1.raw, punch_a2.raw]);
    }
}
