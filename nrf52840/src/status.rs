use chrono::{DateTime, FixedOffset, TimeDelta};
use defmt::info;
use embassy_nrf::temp::Temp as EmbassyNrfTemp;
use embassy_time::Instant;
use heapless::{format, String};
use yaroc_common::{
    at::uart::Tx,
    status::{parse_qlts, MiniCallHome},
};

use crate::{bg77::BG77, bg77_hw::ModemHw, error::Error};

pub trait Temp {
    fn cpu_temperature(&mut self) -> impl core::future::Future<Output = f32>;
}

pub struct NrfTemp {
    temp: EmbassyNrfTemp<'static>,
}

impl NrfTemp {
    pub fn new(temp: EmbassyNrfTemp<'static>) -> Self {
        Self { temp }
    }
}

impl Temp for NrfTemp {
    async fn cpu_temperature(&mut self) -> f32 {
        let temp = self.temp.read().await;
        temp.to_num::<f32>()
    }
}

impl<S: Temp, T: Tx> BG77<S, T> {
    async fn get_modem_time(&mut self) -> crate::Result<DateTime<FixedOffset>> {
        let modem_clock =
            self.simple_call_at("+QLTS=2", None).await?.parse1::<String<25>>([0], None)?;
        parse_qlts(&modem_clock).map_err(yaroc_common::error::Error::into)
    }

    pub async fn current_time(&mut self, cached: bool) -> Option<DateTime<FixedOffset>> {
        if self.boot_time.is_none() || !cached {
            let boot_time = self
                .get_modem_time()
                .await
                .map(|time| {
                    let booted = TimeDelta::milliseconds(Instant::now().as_millis() as i64);
                    time.checked_sub_signed(booted).unwrap()
                })
                .ok()?;
            info!("Boot at {}", format!(30; "{}", boot_time).unwrap().as_str());
            self.boot_time = Some(boot_time);
        }
        self.boot_time.map(|boot_time| {
            let delta = TimeDelta::milliseconds(Instant::now().as_millis() as i64);
            boot_time.checked_add_signed(delta).unwrap()
        })
    }

    async fn battery_state(&mut self) -> Result<(u16, u8), Error> {
        let (bcs, volt) =
            self.simple_call_at("+CBC", None).await?.parse2::<u8, u16>([1, 2], None)?;
        Ok((volt, bcs))
    }

    async fn signal_info(&mut self) -> Result<(i8, i8, u8, i8), Error> {
        let response = self.simple_call_at("+QCSQ", None).await?;
        if response.count_response_values() != Ok(5) {
            return Err(Error::NetworkRegistrationError);
        }
        Ok(response.parse4::<i8, i8, u8, i8>([1, 2, 3, 4])?)
    }

    async fn cellid(&mut self) -> Result<u32, Error> {
        self.simple_call_at("+CEREG?", None)
            .await?
            // TODO: support roaming, that's answer 5
            .parse2::<u32, String<8>>([1, 3], Some(1))
            .map_err(Error::from)
            .and_then(|(_, cell)| u32::from_str_radix(&cell, 16).map_err(|_| Error::ParseError))
    }

    pub async fn mini_call_home(&mut self) -> Option<MiniCallHome> {
        let timestamp = self.current_time(true).await?;
        let cpu_temperature = self.temp.cpu_temperature().await;
        let mut mini_call_home = MiniCallHome::new(timestamp).set_cpu_temperature(cpu_temperature);
        if let Ok((battery_mv, battery_percents)) = self.battery_state().await {
            mini_call_home.set_battery_info(battery_mv, battery_percents);
        }
        if let Ok((rssi_dbm, rsrp_dbm, snr_mult, rsrq_dbm)) = self.signal_info().await {
            let snr_cb = i16::from(snr_mult) * 2 - 200;
            mini_call_home.set_signal_info(snr_cb, rssi_dbm, rsrp_dbm, rsrq_dbm);
        }
        if let Ok(cellid) = self.cellid().await {
            mini_call_home.set_cellid(cellid);
        }

        Some(mini_call_home)
    }
}
