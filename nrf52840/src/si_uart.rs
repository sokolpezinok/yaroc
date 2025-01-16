#![allow(dead_code)]
use chrono::NaiveDate;
use defmt::info;
use embassy_nrf::{gpio::Input, peripherals::UARTE0, uarte::UarteRx};
use embassy_sync::channel::Channel;
use embassy_time::Instant;
use yaroc_common::{punch::SiPunch, RawMutex};

use crate::error::Error;

const LEN: usize = 20;
pub type SiUartChannelType = Channel<RawMutex, Result<SiPunch, Error>, 5>;

pub struct SoftwareSerial {
    io: Input<'static>,
}

impl SoftwareSerial {
    /// Creates new SoftwareSerial instance from a GPIO pin
    pub fn new(io: Input<'static>) -> Self {
        Self { io }
    }

    async fn read(&mut self, buffer: &mut [u8]) {
        // TODO: this only works if it's the only task and there are no interrupts! Needs to be
        // executed using the highest priority.
        for byte in buffer.iter_mut() {
            self.io.wait_for_low().await;
            let time = Instant::now();
            for i in 0..8 {
                cortex_m::asm::delay(1000);
                if self.io.is_high() {
                    *byte |= 1 << i;
                }
            }
            let t1 = time.elapsed();
            self.io.wait_for_high().await;
            info!("Val={}, {}, elapsed={}", byte, t1, time.elapsed());
        }
        info!("Got {} bytes: {}", buffer.len(), buffer);
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

    /// Read 20 bytes of data and convert it into SiPunch.
    ///
    /// Return error if reading from RX or conversion is unsuccessful.
    async fn read(&mut self, today: NaiveDate) -> crate::Result<SiPunch> {
        let mut buf = [0u8; LEN];
        self.rx.read(&mut buf).await.map_err(|_| Error::UartReadError)?;
        Ok(SiPunch::from_raw(buf, today))
    }
}

#[embassy_executor::task]
pub async fn si_uart_reader(mut si_uart: SiUart, si_uart_channel: &'static SiUartChannelType) {
    loop {
        // TODO: get current date
        let date = NaiveDate::from_ymd_opt(2025, 1, 15).unwrap();
        let si_punch = si_uart.read(date).await.unwrap();
        info!("{} punched {} at ...", si_punch.card, si_punch.code);
        si_uart_channel.send(Ok(si_punch)).await;
    }
}
