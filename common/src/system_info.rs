use core::marker::PhantomData;

use crate::{
    bg77_hw::ModemHw,
    error::Error,
    status::{parse_qlts, CellNetworkType, MiniCallHome, SignalInfo},
    RawMutex,
};
use chrono::{DateTime, FixedOffset, TimeDelta};
#[cfg(feature = "defmt")]
use defmt::{error, info};
use embassy_sync::watch::{Receiver, Sender, Watch};
use embassy_time::Instant;
use heapless::{format, String};
#[cfg(not(feature = "defmt"))]
use log::{error, info};

pub trait Temp {
    fn cpu_temperature(&mut self) -> impl core::future::Future<Output = crate::Result<f32>>;
}

#[cfg(feature = "nrf")]
pub struct NrfTemp {
    temp: embassy_nrf::temp::Temp<'static>,
}

#[cfg(feature = "nrf")]
impl NrfTemp {
    pub fn new(temp: embassy_nrf::temp::Temp<'static>) -> Self {
        Self { temp }
    }
}

#[cfg(feature = "nrf")]
impl Temp for NrfTemp {
    async fn cpu_temperature(&mut self) -> crate::Result<f32> {
        let temp = self.temp.read().await;
        Ok(temp.to_num::<f32>())
    }
}

pub static TEMPERATURE: Watch<RawMutex, f32, 1> = Watch::new();

#[derive(Clone, Copy)]
pub struct BatteryInfo {
    pub mv: u16,
    pub percents: u8,
}
pub static BATTERY: Watch<RawMutex, BatteryInfo, 1> = Watch::new();

pub struct SystemInfo<M: ModemHw> {
    temp: Receiver<'static, RawMutex, f32, 1>,
    battery: Receiver<'static, RawMutex, BatteryInfo, 1>,
    battery_sender: Sender<'static, RawMutex, BatteryInfo, 1>,
    boot_time: Option<DateTime<FixedOffset>>,
    _phantom: PhantomData<M>,
}

impl<M: ModemHw> Default for SystemInfo<M> {
    fn default() -> Self {
        Self {
            temp: TEMPERATURE.receiver().unwrap(),
            battery: BATTERY.receiver().unwrap(),
            battery_sender: BATTERY.sender(),
            boot_time: None,
            _phantom: PhantomData,
        }
    }
}

impl<M: ModemHw> SystemInfo<M> {
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

    pub async fn update_battery_state(&self, bg77: &mut M) -> Result<(), Error> {
        let (percents, mv) =
            bg77.simple_call_at("+CBC", None).await?.parse2::<u8, u16>([1, 2], None)?;
        self.battery_sender.send(BatteryInfo { mv, percents });
        Ok(())
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
            let response =
                bg77.simple_call_at("+QCFG=\"celevel\"", None).await?.parse1::<u8>([1], None)?;
            match response {
                0 => CellNetworkType::NbIotEcl0,
                1 => CellNetworkType::NbIotEcl1,
                2 => CellNetworkType::NbIotEcl2,
                _ => return Err(Error::ModemError),
            }
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
        let cpu_temperature = self.temp.try_get();
        let mut mini_call_home = MiniCallHome::new(timestamp);
        if let Some(cpu_temperature) = cpu_temperature {
            mini_call_home.set_cpu_temperature(cpu_temperature);
        }
        if let Some(BatteryInfo { mv, percents }) = self.battery.try_get() {
            mini_call_home.set_battery_info(mv, percents);
        }
        if let Ok(signal_info) = Self::signal_info(bg77).await {
            mini_call_home.set_signal_info(signal_info);
        }
        match Self::cellid(bg77).await {
            Ok(cellid) => mini_call_home.set_cellid(cellid),
            Err(err) => error!("Error while getting cell ID: {}", err),
        }

        Some(mini_call_home)
    }
}
