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

/// SportIdent UART. Reads chunks of 20 bytes.
pub struct SiUart {
    rx: UarteRxWithIdle<'static>,
    buf: [u8; LEN * 5],
    end: usize,
}

impl SiUart {
    /// Creates new SiUart from an UART RX.
    pub fn new(rx: UarteRxWithIdle<'static>) -> Self {
        Self {
            rx,
            buf: [0; LEN * 5],
            end: 0,
        }
    }

    /// Read 20 bytes of SI punch data
    ///
    /// Return error if reading from RX or conversion is unsuccessful.
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

/// A task that reads from the SI UART and sends the punches to the event handler.
#[embassy_executor::task]
pub async fn si_uart_reader(
    mut si_uart: SiUart,
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
