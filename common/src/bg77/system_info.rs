use core::marker::PhantomData;

use crate::{
    RawMutex,
    bg77::hw::ModemHw,
    error::Error,
    status::{
        BATTERY, BatteryInfo, CellNetworkType, CellSignalInfo, MiniCallHome, TEMPERATURE,
        parse_qlts,
    },
};
use chrono::{DateTime, FixedOffset, TimeDelta};
#[cfg(feature = "defmt")]
use defmt::{error, info};
use embassy_sync::watch::{Receiver, Sender};
use embassy_time::Instant;
use heapless::{String, format};
#[cfg(not(feature = "defmt"))]
use log::{error, info};

/// Gathers and provides system information from the Quectel BG77 modem.
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
        let modem_clock = bg77.call_at("+QLTS=2", None).await?.parse1::<String<25>>([0], None)?;
        parse_qlts(&modem_clock)
    }

    /// Returns the current time from the modem.
    ///
    /// The time is fetched from the modem on the first call or when `cached` is false.
    /// Subsequent calls with `cached` as true will return a locally calculated time based on the
    /// boot time and the time elapsed since.
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

    /// Fetches the battery state from the modem and updates the global `BATTERY` status.
    pub async fn update_battery_state(&self, bg77: &mut M) -> Result<(), Error> {
        let (percents, mv) = bg77.call_at("+CBC", None).await?.parse2::<u8, u16>([1, 2], None)?;
        self.battery_sender.send(BatteryInfo { mv, percents });
        Ok(())
    }

    async fn signal_info(bg77: &mut M) -> Result<CellSignalInfo, Error> {
        let response = bg77.call_at("+QCSQ", None).await?;
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
                bg77.call_at("+QCFG=\"celevel\"", None).await?.parse1::<u8>([1], None)?;
            match response {
                0 => CellNetworkType::NbIotEcl0,
                1 => CellNetworkType::NbIotEcl1,
                2 => CellNetworkType::NbIotEcl2,
                _ => return Err(Error::ModemError),
            }
        } else {
            CellNetworkType::LteM
        };
        let cellid = Self::cell_id(bg77)
            .await
            .inspect_err(|err| error!("Error while getting cell ID: {}", err))
            .ok();
        Ok(CellSignalInfo {
            network_type,
            rssi_dbm,
            snr_cb,
            cellid,
        })
    }

    async fn cell_id(bg77: &mut M) -> Result<u32, Error> {
        bg77.call_at("+CEREG?", None)
            .await?
            // TODO: support roaming, that's answer 5
            .parse2::<u32, String<8>>([1, 3], Some(1))
            .and_then(|(_, cell)| u32::from_str_radix(&cell, 16).map_err(|_| Error::ParseError))
    }

    /// Gathers various pieces of system information into a `MiniCallHome` struct.
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

        Some(mini_call_home)
    }
}

#[cfg(feature = "std")]
#[cfg(test)]
mod test {
    use crate::bg77::hw::FakeModem;

    use super::*;

    use embassy_futures::block_on;

    #[test]
    fn test_basic_system_info() {
        let mut bg77 = FakeModem::new(&[
            ("AT+QLTS=2", "+QLTS: \"2024/12/24,10:48:23+04,0\""),
            ("AT+QCSQ", "+QCSQ: \"NBIoT\",-107,-134,35,-20"),
            ("AT+QCFG=\"celevel\"", "+QCFG: \"celevel\",1"),
            ("AT+CEREG?", "+CEREG: 2,1,\"2008\",\"2B2078\",9"),
        ]);

        TEMPERATURE.sender().send(27.0);
        BATTERY.sender().send(BatteryInfo {
            mv: 3967,
            percents: 76,
        });
        let mut system_info = SystemInfo::default();

        let mch = block_on(system_info.mini_call_home(&mut bg77)).unwrap();
        let signal_info = mch.signal_info.unwrap();
        assert_eq!(signal_info.network_type, CellNetworkType::NbIotEcl1);
        assert_eq!(signal_info.rssi_dbm, -107);
        assert_eq!(signal_info.snr_cb, -130);
        assert_eq!(
            signal_info.cellid,
            Some(u32::from_str_radix("2B2078", 16).unwrap())
        );
        assert_eq!(mch.batt_mv, Some(3967));
        assert_eq!(mch.batt_percents, Some(76));
        assert_eq!(mch.cpu_temperature, Some(27.0));
        assert_eq!(
            mch.timestamp,
            DateTime::<FixedOffset>::parse_from_str(
                "2024-12-24 10:48:23+01:00",
                "%Y-%m-%d %H:%M:%S%:z"
            )
            .unwrap()
        );
    }
}
