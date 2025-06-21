use chrono::prelude::*;
use core::fmt;
use femtopb::EnumValue;
#[cfg(feature = "receive")]
use geoutils::Location;
use heapless::String;

use crate::{
    error::Error,
    proto::{self, status, MiniCallHome as MiniCallHomeProto, Status, Timestamp},
};

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

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub enum MacAddress {
    Meshtastic(u32),
    Full(u64),
}

impl TryFrom<&str> for MacAddress {
    type Error = crate::error::Error;

    fn try_from(mac_address: &str) -> crate::Result<Self> {
        match mac_address.len() {
            8 => Ok(MacAddress::Meshtastic(
                u32::from_str_radix(mac_address, 16).map_err(|_| Error::ParseError)?,
            )),
            12 => Ok(MacAddress::Full(
                u64::from_str_radix(mac_address, 16).map_err(|_| Error::ParseError)?,
            )),
            _ => Err(Error::ValueError),
        }
    }
}

impl Default for MacAddress {
    fn default() -> Self {
        Self::Full(0x1234)
    }
}

impl core::fmt::Display for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MacAddress::Meshtastic(mac) => write!(f, "{:08x}", mac),
            MacAddress::Full(mac) => write!(f, "{:012x}", mac),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostInfo {
    pub name: String<20>,
    pub mac_address: MacAddress,
}

impl HostInfo {
    pub fn new(name: &str, mac_address: MacAddress) -> crate::Result<Self> {
        Ok(Self {
            name: name.try_into().map_err(|_| Error::ValueError)?,
            mac_address,
        })
    }
}

/// Cell network type, currently only NB-IoT and LTE-M is supported
#[derive(Default, Debug, PartialEq, Eq)]
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

pub struct SignalInfo {
    pub network_type: CellNetworkType,
    /// RSSI in dBm
    pub rssi_dbm: i8,
    /// SNR in centibells (instead of decibells)
    pub snr_cb: i16,
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
    pub network_type: CellNetworkType,
    pub rssi_dbm: Option<i8>,
    pub snr_cb: Option<i16>, // centibells, 1/10th of decibell
    pub cellid: Option<u32>,
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

    pub fn set_signal_info(&mut self, signal_info: SignalInfo) {
        self.network_type = signal_info.network_type;
        self.snr_cb = Some(signal_info.snr_cb);
        self.rssi_dbm = Some(signal_info.rssi_dbm);
    }

    pub fn set_battery_info(&mut self, battery_mv: u16, battery_percents: u8) {
        self.batt_mv = Some(battery_mv);
        self.batt_percents = Some(battery_percents);
    }

    pub fn set_cpu_temperature(&mut self, cpu_temperature: f32) {
        self.cpu_temperature = Some(cpu_temperature);
    }

    pub fn set_cellid(&mut self, cellid: u32) {
        self.cellid = Some(cellid);
    }

    pub fn to_proto(&self) -> Status {
        let network_type = match self.network_type {
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
                network_type: femtopb::EnumValue::Unknown(network_type as i32),
                signal_dbm: self.rssi_dbm.unwrap_or_default() as i32,
                signal_snr_cb: self.snr_cb.unwrap_or_default() as i32,
                cellid: self.cellid.unwrap_or_default(),
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
        Ok(Self {
            batt_mv: Some(value.millivolts as u16),
            batt_percents: None, // TODO
            network_type,
            rssi_dbm: Some(i8::try_from(value.signal_dbm).map_err(|_| Error::FormatError)?),
            snr_cb: Some(i16::try_from(value.signal_snr_cb).map_err(|_| Error::FormatError)?),
            cellid: if value.cellid > 0 {
                Some(value.cellid)
            } else {
                None
            },
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
