use crate::error::Error;
use embassy_futures::select::{select, Either};
use embassy_nrf::{gpio::Input, peripherals::UARTE0, uarte::UarteRx};
use embassy_sync::channel::{Channel, Sender};
use nrf52840_hal::pac::DWT;
use yaroc_common::{punch::RawPunch, RawMutex};

pub type SiUartChannelType = Channel<RawMutex, Result<RawPunch, Error>, 15>;

pub struct SoftwareSerial {
    io: Input<'static>,
}

impl SoftwareSerial {
    /// Creates new SoftwareSerial instance from a GPIO pin
    pub fn new(io: Input<'static>) -> Self {
        Self { io }
    }

    // TODO: this only works if it's the only task and there are no interrupts! Needs to be
    // executed using the highest priority.
    async fn read(&mut self) -> RawPunch {
        // CPU frequency: 32768 MHz, baud rate 38400, the number of cycles per bit should be
        // 32768000 / 38400 = 853. But it's a different number, almost twice as high.
        const CYCLES_PER_BIT: u32 = 1664;

        let mut buffer = RawPunch::default();
        for byte in buffer.iter_mut() {
            self.io.wait_for_low().await;
            let start_cycles = DWT::cycle_count();
            for i in 0..8 {
                let discrepancy =
                    start_cycles + 200 + (i + 1) * CYCLES_PER_BIT - DWT::cycle_count();
                if discrepancy < CYCLES_PER_BIT * 2 {
                    // The delay function executes actually 50% more cycles, but this is fine, as
                    // we go by discrepancy.
                    cortex_m::asm::delay(discrepancy * 2 / 3);
                }
                if self.io.is_high() {
                    *byte |= 1 << i;
                }
            }
            self.io.wait_for_high().await;
        }
        buffer
    }
}

/// SportIdent UART. Reads chunks of 20 bytes.
pub struct SiUart {
    rx: UarteRx<'static, UARTE0>,
}

impl SiUart {
    /// Creates new SiUart from an UART RX.
    pub fn new(rx: UarteRx<'static, UARTE0>) -> Self {
        Self { rx }
    }

    /// Read 20 bytes of SI punch data
    ///
    /// Return error if reading from RX or conversion is unsuccessful.
    async fn read(&mut self) -> crate::Result<RawPunch> {
        let mut buf = RawPunch::default();
        self.rx.read(&mut buf).await.map_err(|_| Error::UartReadError)?;
        Ok(buf)
    }
}

#[embassy_executor::task]
pub async fn si_uart_reader(
    mut si_uart: SiUart,
    mut software_serial: SoftwareSerial,
    punch_sender: Sender<'static, RawMutex, Result<RawPunch, Error>, 15>,
) {
    loop {
        match select(si_uart.read(), software_serial.read()).await {
            Either::First(res) => match res {
                Err(err) => {
                    punch_sender.send(Err(err)).await;
                }
                Ok(buffer) => {
                    punch_sender.send(Ok(buffer)).await;
                }
            },
            Either::Second(buffer) => {
                punch_sender.send(Ok(buffer)).await;
            }
        }
    }
}
