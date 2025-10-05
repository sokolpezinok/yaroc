//! SI-UART driver.
//!
//! This module is a bit of a misnomer. It doesn't implement the full SI-UART protocol, but rather
//! a simplified version that reads punches from a SportIdent device. It is designed to be used
//! with a single SportIdent device connected to the UART.
//!
//! The module provides a task that reads from the UART and sends the punches to a channel.

use crate::error::Error;
use embassy_nrf::uarte::UarteRxWithIdle;
use embassy_sync::channel::{Channel, Sender};
use yaroc_common::{
    RawMutex,
    punch::{LEN, RawPunch, SiPunch},
};

/// A channel for sending punches from the SI UART to the event handler.
pub type SiUartChannelType = Channel<RawMutex, Result<RawPunch, Error>, 40>;

// TODO: requires DWT which is now disabled
// pub struct SoftwareSerial {
//     io: Input<'static>,
// }
//
// impl SoftwareSerial {
//     /// Creates new SoftwareSerial instance from a GPIO pin
//     pub fn new(io: Input<'static>) -> Self {
//         Self { io }
//     }
//
//     // TODO: this only works if it's the only task and there are no interrupts! Needs to be
//     // executed using the highest priority.
//     async fn read(&mut self) -> RawPunch {
//         // CPU frequency: 32768 MHz, baud rate 38400, the number of cycles per bit should be
//         // 32768000 / 38400 = 853. But it's a different number, almost twice as high.
//         const CYCLES_PER_BIT: u32 = 1664;
//
//         let mut buffer = RawPunch::default();
//         for byte in buffer.iter_mut() {
//             self.io.wait_for_low().await;
//             let start_cycles = DWT::cycle_count();
//             for i in 0..8 {
//                 let discrepancy =
//                     start_cycles + 200 + (i + 1) * CYCLES_PER_BIT - DWT::cycle_count();
//                 if discrepancy < CYCLES_PER_BIT * 2 {
//                     // The delay function executes actually 50% more cycles, but this is fine, as
//                     // we go by discrepancy.
//                     cortex_m::asm::delay(discrepancy * 2 / 3);
//                 }
//                 if self.io.is_high() {
//                     *byte |= 1 << i;
//                 }
//             }
//             self.io.wait_for_high().await;
//         }
//         buffer
//     }
// }

/// A trait for reading from a UART until it's idle.
pub trait RxWithIdle {
    /// Read from UART until it's idle. Return the number of read bytes.
    fn read_until_idle(
        &mut self,
        buf: &mut [u8],
    ) -> impl core::future::Future<Output = crate::Result<usize>>;
}

/// Implementation of `RxWithIdle` for `UarteRxWithIdle`.
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
    async fn read(&mut self) -> crate::Result<RawPunch> {
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

/// A task that reads from the SI UART and sends punches to a channel.
///
/// This task continuously reads from the SI-UART and sends the parsed punches to the provided
/// channel. If an error occurs during reading or parsing, the error is sent to the channel.
#[embassy_executor::task]
pub async fn si_uart_reader(
    mut si_uart: SiUart<UarteRxWithIdle<'static>>,
    punch_sender: Sender<'static, RawMutex, Result<RawPunch, Error>, 40>,
) {
    loop {
        match si_uart.read().await {
            Err(err) => {
                punch_sender.send(Err(err)).await;
            }
            Ok(buffer) => {
                punch_sender.send(Ok(buffer)).await;
            }
        }
    }
}
