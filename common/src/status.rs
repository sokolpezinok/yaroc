use chrono::prelude::*;
use embassy_sync::watch::Watch;
use femtopb::EnumValue;
#[cfg(feature = "receive")]
use geoutils::Location;

use crate::RawMutex;
use crate::{
    error::Error,
    proto::{self, MiniCallHome as MiniCallHomeProto, Status, Timestamp, status},
};

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

/// Parses the output of AT+QLTS=2 command into a date+time.
pub fn parse_qlts(modem_clock: &str) -> Result<DateTime<FixedOffset>, Error> {
    let naive_date = NaiveDateTime::parse_from_str(&modem_clock[..19], "%Y/%m/%d,%H:%M:%S")
        .map_err(|_| Error::ParseError)?;

    let offset = str::parse::<u8>(&modem_clock[20..22]).map_err(|_| Error::ParseError)?;
    Ok(naive_date
        .and_local_timezone(
            FixedOffset::east_opt(i32::from(offset) * 900).ok_or(Error::ParseError)?,
        )
        .unwrap()
        .fixed_offset())
}

/// Cell network type, currently only NB-IoT and LTE-M is supported
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum CellNetworkType {
    #[default]
    Unknown,
    // NB-IoT can be on 3 different extended coverage levels (ECL): 0, 1 and 2.
    // We encode it in this enum to save a few bytes.
    /// NB-IoT ECL 0
    NbIotEcl0,
    /// NB-IoT ECL 1
    NbIotEcl1,
    /// NB-IoT ECL 2
    NbIotEcl2,
    /// LTE-M
    LteM,
    /// UMTS
    Umts,
    /// LTE
    Lte,
}

impl From<proto::CellNetworkType> for CellNetworkType {
    fn from(value: proto::CellNetworkType) -> Self {
        match value {
            proto::CellNetworkType::Lte => CellNetworkType::Lte,
            proto::CellNetworkType::Umts => CellNetworkType::Umts,
            proto::CellNetworkType::LteM => CellNetworkType::LteM,
            proto::CellNetworkType::NbIotEcl0 => CellNetworkType::NbIotEcl0,
            proto::CellNetworkType::NbIotEcl1 => CellNetworkType::NbIotEcl1,
            proto::CellNetworkType::NbIotEcl2 => CellNetworkType::NbIotEcl2,
            _ => CellNetworkType::Unknown,
        }
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct CellSignalInfo {
    pub network_type: CellNetworkType,
    /// RSSI in dBm
    pub rssi_dbm: i8,
    /// SNR in centibells (instead of decibells)
    pub snr_cb: i16,
    /// Cell ID
    pub cellid: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Position {
    pub lat: f32,
    pub lon: f32,
    pub elevation: i32,
    pub timestamp: DateTime<FixedOffset>,
}

impl Position {
    pub fn new(lat: f32, lon: f32, timestamp: DateTime<FixedOffset>) -> Self {
        Self {
            lat,
            lon,
            elevation: 0,
            timestamp,
        }
    }

    #[cfg(feature = "receive")]
    pub fn distance_m(&self, other: &Position) -> crate::Result<f64> {
        let me = Location::new(self.lat, self.lon);
        let other = Location::new(other.lat, other.lon);
        Ok(me.distance_to(&other).map_err(|_| Error::ValueError)?.meters())
    }
}

#[derive(Default, Debug)]
pub struct MiniCallHome {
    pub signal_info: Option<CellSignalInfo>,
    pub batt_mv: Option<u16>,
    pub batt_percents: Option<u8>,
    pub cpu_temperature: Option<f32>,
    pub cpu_freq: Option<u32>,
    pub timestamp: DateTime<FixedOffset>,
}

impl MiniCallHome {
    pub fn new(timestamp: DateTime<FixedOffset>) -> Self {
        Self {
            timestamp,
            ..Default::default()
        }
    }

    pub fn set_signal_info(&mut self, signal_info: CellSignalInfo) {
        self.signal_info = Some(signal_info);
    }

    pub fn set_battery_info(&mut self, battery_mv: u16, battery_percents: u8) {
        self.batt_mv = Some(battery_mv);
        self.batt_percents = Some(battery_percents);
    }

    pub fn set_cpu_temperature(&mut self, cpu_temperature: f32) {
        self.cpu_temperature = Some(cpu_temperature);
    }

    pub fn to_proto(self) -> Status<'static> {
        let signal_info = &self.signal_info.unwrap_or_default();
        let network_type = match signal_info.network_type {
            CellNetworkType::Lte => proto::CellNetworkType::Lte,
            CellNetworkType::Umts => proto::CellNetworkType::Umts,
            CellNetworkType::LteM => proto::CellNetworkType::LteM,
            CellNetworkType::NbIotEcl0 => proto::CellNetworkType::NbIotEcl0,
            CellNetworkType::NbIotEcl1 => proto::CellNetworkType::NbIotEcl1,
            CellNetworkType::NbIotEcl2 => proto::CellNetworkType::NbIotEcl2,
            _ => proto::CellNetworkType::UnknownNetworkType,
        };
        Status {
            msg: Some(status::Msg::MiniCallHome(MiniCallHomeProto {
                freq: 32,
                millivolts: self.batt_mv.unwrap_or_default() as u32,
                network_type: femtopb::EnumValue::Known(network_type),
                signal_dbm: i32::from(signal_info.rssi_dbm),
                signal_snr_cb: i32::from(signal_info.snr_cb),
                cellid: signal_info.cellid.unwrap_or_default(),
                time: Some(Timestamp {
                    millis_epoch: self.timestamp.timestamp_millis() as u64,
                    ..Default::default()
                }),
                cpu_temperature: self.cpu_temperature.unwrap_or_default(),
                ..Default::default()
            })),
            ..Default::default()
        }
    }
}

impl TryFrom<MiniCallHomeProto<'_>> for MiniCallHome {
    type Error = crate::error::Error;

    fn try_from(value: MiniCallHomeProto) -> crate::Result<Self> {
        // TODO: is missing timestamp such a big problem? Could we remove the question mark
        // here?
        let timestamp_millis = value.time.ok_or(Error::FormatError)?.millis_epoch;
        let timestamp = DateTime::from_timestamp_millis(timestamp_millis as i64)
            .ok_or(Error::FormatError)?
            .into();
        let network_type = match value.network_type {
            EnumValue::Known(network_type) => network_type.into(),
            EnumValue::Unknown(_) => CellNetworkType::Unknown,
        };
        let signal_info = CellSignalInfo {
            network_type,
            rssi_dbm: i8::try_from(value.signal_dbm).map_err(|_| Error::FormatError)?,
            snr_cb: i16::try_from(value.signal_snr_cb).map_err(|_| Error::FormatError)?,
            cellid: if value.cellid > 0 {
                Some(value.cellid)
            } else {
                None
            },
        };

        Ok(Self {
            signal_info: Some(signal_info),
            batt_mv: Some(value.millivolts as u16),
            batt_percents: None, // TODO
            cpu_temperature: Some(value.cpu_temperature),
            timestamp,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod test {
    use chrono::{NaiveDate, NaiveTime};

    use super::parse_qlts;

    #[test]
    fn test_cclk() {
        let dt = parse_qlts("2024/11/25,22:12:11+04extra").unwrap();
        let naive_dt = dt.naive_local();
        assert_eq!(
            naive_dt.date(),
            NaiveDate::from_ymd_opt(2024, 11, 25).unwrap()
        );
        assert_eq!(
            naive_dt.time(),
            NaiveTime::from_hms_opt(22, 12, 11).unwrap()
        );
        assert_eq!(dt.offset().local_minus_utc(), 3600);
    }
}
