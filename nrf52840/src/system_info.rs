use core::marker::PhantomData;

use chrono::{DateTime, FixedOffset, TimeDelta};
use defmt::info;
use embassy_nrf::temp::Temp as EmbassyNrfTemp;
use embassy_time::Instant;
use heapless::{format, String};
use yaroc_common::status::{parse_qlts, CellNetworkType, MiniCallHome, SignalInfo};

use crate::{bg77_hw::ModemHw, error::Error};

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

pub struct FakeTemp {
    pub t: f32,
}

impl Temp for FakeTemp {
    async fn cpu_temperature(&mut self) -> f32 {
        self.t
    }
}

pub struct SystemInfo<M: ModemHw, T: Temp> {
    temp: T,
    boot_time: Option<DateTime<FixedOffset>>,
    _phantom: PhantomData<M>,
}

impl<M: ModemHw, T: Temp> SystemInfo<M, T> {
    pub fn new(temp: T) -> Self {
        Self {
            temp,
            boot_time: None,
            _phantom: PhantomData,
        }
    }

    async fn get_modem_time(bg77: &mut impl ModemHw) -> crate::Result<DateTime<FixedOffset>> {
        let modem_clock =
            bg77.simple_call_at("+QLTS=2", None).await?.parse1::<String<25>>([0], None)?;
        parse_qlts(&modem_clock)
    }

    pub async fn current_time(
        &mut self,
        bg77: &mut M,
        cached: bool,
    ) -> Option<DateTime<FixedOffset>> {
        if self.boot_time.is_none() || !cached {
            let boot_time = Self::get_modem_time(bg77)
                .await
                .map(|time| {
                    let booted = TimeDelta::milliseconds(Instant::now().as_millis() as i64);
                    time.checked_sub_signed(booted).unwrap()
                })
                .ok()?;
            info!("Boot at {}", format!(30; "{}", boot_time).unwrap());
            self.boot_time = Some(boot_time);
        }
        self.boot_time.map(|boot_time| {
            let delta = TimeDelta::milliseconds(Instant::now().as_millis() as i64);
            boot_time.checked_add_signed(delta).unwrap()
        })
    }

    async fn battery_state(bg77: &mut M) -> Result<(u16, u8), Error> {
        let (bcs, volt) =
            bg77.simple_call_at("+CBC", None).await?.parse2::<u8, u16>([1, 2], None)?;
        Ok((volt, bcs))
    }

    async fn signal_info(bg77: &mut M) -> Result<SignalInfo, Error> {
        let response = bg77.simple_call_at("+QCSQ", None).await?;
        if response.count_response_values() != Ok(5) {
            return Err(Error::NetworkRegistrationError);
        }
        let (network, mut rssi_dbm, rsrp_dbm, snr_mult, rsrq_dbm) =
            response.parse5::<String<10>, i8, i16, u8, i8>([0, 1, 2, 3, 4])?;
        let snr_cb = i16::from(snr_mult) * 2 - 200;
        if rssi_dbm == 0 {
            rssi_dbm = (rsrp_dbm - i16::from(rsrq_dbm)) as i8; // TODO: error if not i8
        }
        let network_type = if network == "NBIoT" {
            // TODO: add ECL detection
            CellNetworkType::NbIotEcl0
        } else {
            CellNetworkType::LteM
        };
        Ok(SignalInfo {
            network_type,
            rssi_dbm,
            snr_cb,
        })
    }

    async fn cellid(bg77: &mut M) -> Result<u32, Error> {
        bg77.simple_call_at("+CEREG?", None)
            .await?
            // TODO: support roaming, that's answer 5
            .parse2::<u32, String<8>>([1, 3], Some(1))
            .and_then(|(_, cell)| u32::from_str_radix(&cell, 16).map_err(|_| Error::ParseError))
    }

    pub async fn mini_call_home(&mut self, bg77: &mut M) -> Option<MiniCallHome> {
        let timestamp = self.current_time(bg77, true).await?;
        let cpu_temperature = self.temp.cpu_temperature().await;
        let mut mini_call_home = MiniCallHome::new(timestamp).set_cpu_temperature(cpu_temperature);
        if let Ok((battery_mv, battery_percents)) = Self::battery_state(bg77).await {
            mini_call_home.set_battery_info(battery_mv, battery_percents);
        }
        if let Ok(signal_info) = Self::signal_info(bg77).await {
            mini_call_home.set_signal_info(signal_info);
        }
        match Self::cellid(bg77).await {
            Ok(cellid) => mini_call_home.set_cellid(cellid),
            Err(err) => defmt::error!("Error while getting cell ID: {}", err),
        }

        Some(mini_call_home)
    }
}
