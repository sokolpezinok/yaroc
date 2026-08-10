use core::marker::PhantomData;

use crate::{
    RawMutex,
    at::uart::AtUartTrait,
    bg77::modem_manager::RegistrationError,
    error::Error,
    status::{BATTERY, BatteryInfo, CellNetworkType, CellSignalInfo, MiniCallHome, TEMPERATURE},
};
use chrono::{DateTime, FixedOffset, TimeDelta};
#[cfg(feature = "defmt")]
use defmt::{error, info};
use embassy_sync::watch::{Receiver, Watch};
use embassy_time::Instant;
use heapless::{String, format};
#[cfg(not(feature = "defmt"))]
use log::{error, info};

pub(crate) static BOOT_TIME: Watch<RawMutex, DateTime<FixedOffset>, 1> = Watch::new();

/// Returns the calendar time corresponding to the given `instant`,
/// based on the synchronized boot time. Returns `None` if the boot time has not been synchronized yet.
pub fn time_from_instant(instant: Instant) -> DateTime<FixedOffset> {
    let boot_time = BOOT_TIME.receiver().unwrap().try_get().unwrap_or_default();
    let delta = TimeDelta::milliseconds(instant.as_millis() as i64);
    boot_time.checked_add_signed(delta).unwrap()
}

/// Gathers and provides system information from the Quectel BG77 modem.
pub struct SystemInfo<M: AtUartTrait> {
    temp: Receiver<'static, RawMutex, f32, 1>,
    battery: Receiver<'static, RawMutex, BatteryInfo, 1>,
    _phantom: PhantomData<M>,
}

impl<M: AtUartTrait> Default for SystemInfo<M> {
    fn default() -> Self {
        Self {
            temp: TEMPERATURE.receiver().unwrap(),
            battery: BATTERY.receiver().unwrap(),
            _phantom: PhantomData,
        }
    }
}

impl<M: AtUartTrait> SystemInfo<M> {
    /// Parses the date and time from the output of the AT+QLTS=2 command.
    ///
    /// Expected format: `"YYYY/MM/DD,HH:MM:SS±ZZ,D"` (e.g. `"2024/12/24,10:48:23+04,0"`)
    /// - `[0..4]`: Year (`YYYY`)
    /// - `[5..7]`: Month (`MM`)
    /// - `[8..10]`: Day (`DD`)
    /// - `[11..13]`: Hour (`HH`)
    /// - `[14..16]`: Minute (`MM`)
    /// - `[17..19]`: Second (`SS`)
    /// - `[20..22]`: Timezone offset in 15-minute intervals (`ZZ`)
    fn parse_qlts(modem_clock: &str) -> Result<DateTime<FixedOffset>, Error> {
        if modem_clock.len() < 22 {
            return Err(Error::ParseError);
        }
        let year: i32 = str::parse(&modem_clock[0..4]).map_err(|_| Error::ParseError)?;
        let month: u32 = str::parse(&modem_clock[5..7]).map_err(|_| Error::ParseError)?;
        let day: u32 = str::parse(&modem_clock[8..10]).map_err(|_| Error::ParseError)?;
        let hour: u32 = str::parse(&modem_clock[11..13]).map_err(|_| Error::ParseError)?;
        let min: u32 = str::parse(&modem_clock[14..16]).map_err(|_| Error::ParseError)?;
        let sec: u32 = str::parse(&modem_clock[17..19]).map_err(|_| Error::ParseError)?;

        let naive_date = chrono::NaiveDate::from_ymd_opt(year, month, day)
            .ok_or(Error::ParseError)?
            .and_hms_opt(hour, min, sec)
            .ok_or(Error::ParseError)?;

        let offset = str::parse::<u8>(&modem_clock[20..22]).map_err(|_| Error::ParseError)?;
        Ok(naive_date
            .and_local_timezone(
                FixedOffset::east_opt(i32::from(offset) * 900).ok_or(Error::ParseError)?,
            )
            .unwrap()
            .fixed_offset())
    }

    /// Gets modem time from the QLTS command
    async fn get_modem_time(bg77: &mut M) -> crate::Result<DateTime<FixedOffset>> {
        let modem_clock = bg77.call_at("+QLTS=2", None).await?.parse1::<String<25>>([0])?;
        Self::parse_qlts(&modem_clock)
    }

    /// Returns the current time from the modem.
    ///
    /// The time is fetched from the modem on the first call or when `cached` is false.
    /// Subsequent calls with `cached` as true will return a locally calculated time based on the
    /// boot time and the time elapsed since.
    pub async fn current_time(bg77: &mut M, cached: bool) -> Option<DateTime<FixedOffset>> {
        let boot_time = BOOT_TIME.receiver().unwrap().try_get();
        if boot_time.is_none() || !cached {
            let boot_time = Self::get_modem_time(bg77)
                .await
                .map(|time| {
                    let booted = TimeDelta::milliseconds(Instant::now().as_millis() as i64);
                    time.checked_sub_signed(booted).unwrap()
                })
                .ok()?;
            info!("Boot at {}", format!(30; "{}", boot_time).unwrap());
            BOOT_TIME.sender().send(boot_time);
        }
        BOOT_TIME.receiver().unwrap().try_get().map(|boot_time| {
            let delta = TimeDelta::milliseconds(Instant::now().as_millis() as i64);
            boot_time.checked_add_signed(delta).unwrap()
        })
    }

    async fn signal_info(bg77: &mut M) -> Result<CellSignalInfo, RegistrationError> {
        let response = bg77.call_at("+QCSQ", None).await?;
        if response.count_response_values() != Ok(5) {
            return Err(RegistrationError::NoNetworkService);
        }
        let (network, rsrp_dbm, snr_mult, _) =
            response.parse4::<String<10>, i16, u8, i8>([0, 2, 3, 4])?;
        let snr_cb = i16::from(snr_mult) * 2 - 200;
        let network_type = if network == "NBIoT" {
            let response =
                bg77.call_at("+QCFG=\"celevel\"", None).await.and_then(|r| r.parse1::<u8>([1]));
            match response {
                Ok(0) => CellNetworkType::NbIotEcl0,
                Ok(1) => CellNetworkType::NbIotEcl1,
                Ok(2) => CellNetworkType::NbIotEcl2,
                // Rather than returning an error, we default to ECL 0.
                _ => CellNetworkType::NbIotEcl0,
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
            rsrp_dbm,
            snr_cb,
            cellid,
        })
    }

    async fn cell_id(bg77: &mut M) -> Result<u32, Error> {
        let (stat, cell) =
            bg77.call_at("+CEREG?", None).await?.parse2::<u8, String<8>>([1, 3], None)?;
        if stat == 1 || stat == 5 {
            u32::from_str_radix(&cell, 16).map_err(|_| Error::ParseError)
        } else {
            Err(RegistrationError::NoNetworkService.into())
        }
    }

    /// Gathers various pieces of system information into a `MiniCallHome` struct.
    pub async fn mini_call_home(&mut self, bg77: &mut M) -> MiniCallHome {
        let timestamp = Self::current_time(bg77, true).await;
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

        mini_call_home
    }
}

#[cfg(feature = "std")]
#[cfg(test)]
mod test {
    use crate::at::fake_modem::FakeModem;
    use chrono::{NaiveDate, NaiveTime};

    use super::*;

    use embassy_futures::block_on;
    use embassy_sync::mutex::Mutex;

    static TEST_MUTEX: Mutex<RawMutex, ()> = Mutex::new(());

    #[test]
    fn test_basic_system_info() {
        let _lock = block_on(TEST_MUTEX.lock());
        BOOT_TIME.sender().clear();
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

        let mch = block_on(system_info.mini_call_home(&mut bg77));
        let signal_info = mch.signal_info.unwrap();
        assert_eq!(signal_info.network_type, CellNetworkType::NbIotEcl1);
        assert_eq!(signal_info.rsrp_dbm, -134);
        assert_eq!(signal_info.snr_cb, -130);
        assert_eq!(
            signal_info.cellid,
            Some(u32::from_str_radix("2B2078", 16).unwrap())
        );
        assert_eq!(mch.batt_mv, Some(3967));
        assert_eq!(mch.batt_percents, Some(76));
        assert_eq!(mch.cpu_temperature, Some(27.0));
        assert_eq!(
            mch.timestamp.unwrap(),
            DateTime::<FixedOffset>::parse_from_str(
                "2024-12-24 10:48:23+01:00",
                "%Y-%m-%d %H:%M:%S%:z"
            )
            .unwrap()
        );
    }

    #[test]
    fn test_mini_call_home_no_timestamp() {
        let _lock = block_on(TEST_MUTEX.lock());
        BOOT_TIME.sender().clear();
        let mut bg77 = FakeModem::new(&[
            ("AT+QLTS=2", ""),
            ("AT+QCSQ", "+QCSQ: \"eMTC\",-100,-90,110,-120"),
            ("AT+CEREG?", "+CEREG: 2,1,\"2008\",\"2B2078\",9"),
        ]);
        let mut system_info = SystemInfo::default();
        let mch = block_on(system_info.mini_call_home(&mut bg77));
        assert!(mch.timestamp.is_none());
        assert_eq!(mch.signal_info.unwrap().rsrp_dbm, -90);
    }

    #[test]
    fn test_qlts() {
        let datetime = SystemInfo::<FakeModem>::parse_qlts("2024/11/25,22:12:11+04extra").unwrap();
        let naive_dt = datetime.naive_local();
        assert_eq!(
            naive_dt.date(),
            NaiveDate::from_ymd_opt(2024, 11, 25).unwrap()
        );
        assert_eq!(
            naive_dt.time(),
            NaiveTime::from_hms_opt(22, 12, 11).unwrap()
        );
        assert_eq!(datetime.offset().local_minus_utc(), 3600);
    }

    #[test]
    fn test_time_from_instant() {
        let _lock = block_on(TEST_MUTEX.lock());
        BOOT_TIME.sender().clear();

        // Returns time in 1970 when boot time is not synchronized.
        let instant = Instant::from_secs(5);
        let expected = DateTime::parse_from_rfc3339("1970-01-01T00:00:05+00:00").unwrap();
        assert_eq!(time_from_instant(instant), expected);

        // Returns correct time when boot time is set
        let boot_time = DateTime::parse_from_rfc3339("2026-07-17T18:00:00+02:00").unwrap();
        BOOT_TIME.sender().send(boot_time);

        let instant = Instant::from_secs(5);
        let calculated = time_from_instant(instant);
        let expected = DateTime::parse_from_rfc3339("2026-07-17T18:00:05+02:00").unwrap();
        assert_eq!(calculated, expected);
    }

    #[test]
    fn test_roaming_cell_id() {
        let _lock = block_on(TEST_MUTEX.lock());
        let mut bg77 = FakeModem::new(&[("AT+CEREG?", "+CEREG: 2,5,\"2008\",\"2B2078\",9")]);

        let cell_id = block_on(SystemInfo::<FakeModem>::cell_id(&mut bg77)).unwrap();
        assert_eq!(cell_id, u32::from_str_radix("2B2078", 16).unwrap());
    }

    #[test]
    fn test_nbiot_celevel_fallback() {
        let _lock = block_on(TEST_MUTEX.lock());

        // Case 1: +QCFG="celevel" fails / returns empty response -> defaults to NbIotEcl0
        let mut bg77_fail = FakeModem::new(&[
            ("AT+QCSQ", "+QCSQ: \"NBIoT\",-107,-134,35,-20"),
            ("AT+QCFG=\"celevel\"", ""),
            ("AT+CEREG?", "+CEREG: 2,1,\"2008\",\"2B2078\",9"),
        ]);
        let signal_info_fail =
            block_on(SystemInfo::<FakeModem>::signal_info(&mut bg77_fail)).unwrap();
        assert_eq!(signal_info_fail.network_type, CellNetworkType::NbIotEcl0);

        // Case 2: +QCFG="celevel" returns an unexpected level (e.g. 3) -> defaults to NbIotEcl0
        let mut bg77_invalid = FakeModem::new(&[
            ("AT+QCSQ", "+QCSQ: \"NBIoT\",-107,-134,35,-20"),
            ("AT+QCFG=\"celevel\"", "+QCFG: \"celevel\",3"),
            ("AT+CEREG?", "+CEREG: 2,1,\"2008\",\"2B2078\",9"),
        ]);
        let signal_info_invalid =
            block_on(SystemInfo::<FakeModem>::signal_info(&mut bg77_invalid)).unwrap();
        assert_eq!(signal_info_invalid.network_type, CellNetworkType::NbIotEcl0);
    }

    #[test]
    fn test_qcsq_noservice() {
        let _lock = block_on(TEST_MUTEX.lock());

        let mut bg77 = FakeModem::new(&[("AT+QCSQ", "+QCSQ: \"NOSERVICE\"")]);
        let res = block_on(SystemInfo::<FakeModem>::signal_info(&mut bg77));
        assert_eq!(res, Err(RegistrationError::NoNetworkService));
    }
}
