#![allow(dead_code)]
use chrono::NaiveDate;
use embassy_nrf::{peripherals::UARTE0, uarte::UarteRx};
use embassy_sync::{channel::Channel, mutex::Mutex};
use yaroc_common::punch::SiPunch;

use crate::error::Error;

#[cfg(all(target_abi = "eabihf", target_os = "none"))]
type RawMutex = embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
#[cfg(not(all(target_abi = "eabihf", target_os = "none")))]
type RawMutex = embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
pub type SiUartType = Mutex<RawMutex, Option<SiUart>>;

const LEN: usize = 20;
pub type SiUartChannelType = Channel<RawMutex, Result<SiPunch, Error>, 5>;

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
pub async fn si_uart_reader(
    si_uart_mutex: &'static SiUartType,
    si_uart_channel: &'static SiUartChannelType,
) {
    let mut si_uart = si_uart_mutex.lock().await;
    let si_uart = si_uart.as_mut().unwrap();
    loop {
        // TODO: get current date
        let date = NaiveDate::from_ymd_opt(2024, 12, 9).unwrap();
        let si_punch = si_uart.read(date).await;
        si_uart_channel.send(si_punch).await;
    }
}
