#![allow(dead_code)]
use chrono::NaiveDate;
use common::punch::SiPunch;
use embassy_nrf::{peripherals::UARTE0, uarte::UarteRx};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;

use crate::error::Error;

pub type SiUartType = Mutex<ThreadModeRawMutex, Option<SiUart>>;

// SportIdent UART. Reads chunks of 20 bytes.
pub struct SiUart {
    rx: UarteRx<'static, UARTE0>,
}

const LEN: usize = 20;

impl SiUart {
    pub fn new(rx: UarteRx<'static, UARTE0>) -> Self {
        Self { rx }
    }

    async fn read(&mut self, today: NaiveDate) -> crate::Result<SiPunch> {
        let mut buf = [0u8; LEN];
        self.rx.read(&mut buf).await.map_err(|_| Error::UartReadError)?;
        Ok(SiPunch::from_raw(buf, today))
    }
}
