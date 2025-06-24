extern crate std;

use crate::error::Error;
use crate::proto::status::Msg;
use crate::proto::{DeviceEvent, Disconnected, EventType, Status};
use crate::status::{CellNetworkType, HostInfo, MacAddress, MiniCallHome};
use chrono::prelude::*;
use chrono::{DateTime, Duration};
use femtopb::EnumValue;
use std::borrow::ToOwned;
use std::fmt;
use std::string::String;

pub enum CellularLogMessage {
    Disconnected(String, String),
    MCH(MiniCallHomeLog),
    DeviceEvent(String, String, bool),
}

impl fmt::Display for CellularLogMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CellularLogMessage::MCH(mch) => write!(f, "{}", mch),
            CellularLogMessage::Disconnected(hostname, client_name) => {
                write!(f, "{hostname} disconnected client: {client_name}")
            }
            CellularLogMessage::DeviceEvent(hostname, port, added) => {
                let event_type = if *added { "added" } else { "removed" };
                write!(f, "{hostname} {port} {event_type}")
            }
        }
    }
}

impl CellularLogMessage {
    pub fn from_proto(
        status: Status,
        mac_addr: MacAddress,
        hostname: &str,
        tz: &impl TimeZone,
    ) -> crate::Result<Self> {
        match status.msg {
            Some(Msg::Disconnected(Disconnected { client_name, .. })) => Ok(
                CellularLogMessage::Disconnected(hostname.to_owned(), client_name.to_owned()),
            ),
            Some(Msg::MiniCallHome(mch)) => {
                let now = Local::now().with_timezone(tz);
                let log_message =
                    MiniCallHomeLog::new(hostname, mac_addr, now.fixed_offset(), mch)?;
                Ok(CellularLogMessage::MCH(log_message))
            }
            Some(Msg::DevEvent(DeviceEvent { port, r#type, .. })) => {
                if let EnumValue::Unknown(_) = r#type {
                    return Err(Error::FormatError);
                }
                Ok(CellularLogMessage::DeviceEvent(
                    hostname.to_owned(),
                    port.to_owned(),
                    r#type == EnumValue::Known(EventType::Added),
                ))
            }
            _ => Err(Error::FormatError),
        }
    }
}

pub struct MiniCallHomeLog {
    pub mini_call_home: MiniCallHome,
    pub host_info: HostInfo,
    pub latency: Duration,
}

impl MiniCallHomeLog {
    pub fn new(
        name: &str,
        mac_address: MacAddress,
        now: DateTime<FixedOffset>,
        mch_proto: crate::proto::MiniCallHome,
    ) -> crate::Result<Self> {
        let mut mch: MiniCallHome = mch_proto.try_into()?;
        mch.timestamp = mch.timestamp.with_timezone(now.offset());
        Ok(Self {
            latency: now - mch.timestamp,
            mini_call_home: mch,
            host_info: HostInfo {
                name: name.try_into().map_err(|_| Error::ValueError)?,
                mac_address,
            },
        })
    }
}

impl fmt::Display for MiniCallHomeLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let timestamp = self.mini_call_home.timestamp.format("%H:%M:%S");
        write!(f, "{} {timestamp}:", self.host_info.name)?;
        if let Some(temperature) = &self.mini_call_home.cpu_temperature {
            write!(f, " {temperature:.1}°C")?;
        }
        if let Some(rssi_dbm) = &self.mini_call_home.rssi_dbm {
            write!(f, ", RSSI{rssi_dbm:5}")?;
            if let Some(snr_cb) = &self.mini_call_home.snr_cb {
                write!(f, " SNR{:5.1}", f32::from(*snr_cb) / 10.)?;
            }
            let network_type = match self.mini_call_home.network_type {
                CellNetworkType::NbIotEcl0 => "NB ECL0",
                CellNetworkType::NbIotEcl1 => "NB ECL1",
                CellNetworkType::NbIotEcl2 => "NB ECL2",
                CellNetworkType::LteM => "LTE-M",
                CellNetworkType::Umts => "UMTS",
                CellNetworkType::Lte => "LTE",
                _ => "",
            };
            write!(f, " {network_type:>7}")?;
        }
        if let Some(cellid) = &self.mini_call_home.cellid {
            write!(f, ", cell {cellid:X}")?;
        }
        if let Some(batt_mv) = self.mini_call_home.batt_mv {
            write!(f, ", {:.2}V", f32::from(batt_mv) / 1000.0)?;
        }
        let secs = self.latency.num_milliseconds() as f64 / 1000.0;
        write!(f, ", lat. {:4.2}s", secs)
    }
}

#[cfg(test)]
mod test_logs {
    use super::*;
    use crate::proto::{MiniCallHome, Timestamp};
    use femtopb::EnumValue::Known;
    use std::format;

    #[test]
    fn test_cellular_dbm() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-29T17:40:43+01:00").unwrap().into();
        let log_message = MiniCallHomeLog {
            mini_call_home: crate::status::MiniCallHome {
                batt_mv: Some(1260),
                network_type: CellNetworkType::NbIotEcl0,
                rssi_dbm: Some(-87),
                snr_cb: Some(70),
                cellid: Some(2580590),
                cpu_temperature: Some(51.54),
                timestamp,
                ..Default::default()
            },
            host_info: HostInfo {
                name: "spe01".try_into().unwrap(),
                mac_address: MacAddress::Full(0x1234),
            },
            latency: Duration::milliseconds(1390),
        };
        assert_eq!(
            format!("{log_message}"),
            "spe01 17:40:43: 51.5°C, RSSI  -87 SNR  7.0 NB ECL0, cell 27606E, 1.26V, lat. 1.39s"
        );
    }

    #[test]
    fn test_cellular_logmessage_disconnected() {
        let log_message_disconnected =
            CellularLogMessage::Disconnected("spe01".to_owned(), "SIM7020-spe01".to_owned());
        assert_eq!(
            format!("{log_message_disconnected}"),
            "spe01 disconnected client: SIM7020-spe01"
        );

        let log_message_event =
            CellularLogMessage::DeviceEvent("spe01".to_owned(), "/dev/ttyUSB0".to_owned(), true);
        assert_eq!(format!("{log_message_event}"), "spe01 /dev/ttyUSB0 added");
    }

    #[test]
    fn test_cellular_logmessage_fromproto() {
        let timestamp = Timestamp {
            millis_epoch: 1706523131_124, // 2024-01-29T11:12:11.124+01:00
            ..Default::default()
        };

        let status = Status {
            msg: Some(Msg::MiniCallHome(MiniCallHome {
                cpu_temperature: 47.0,
                millivolts: 3847,
                network_type: Known(crate::proto::CellNetworkType::LteM),
                signal_dbm: -80,
                signal_snr_cb: 120,
                time: Some(timestamp),
                ..Default::default()
            })),
            ..Default::default()
        };
        let tz = FixedOffset::east_opt(3600).unwrap();
        let cell_log_msg =
            CellularLogMessage::from_proto(status, MacAddress::default(), "spe01", &tz)
                .expect("MiniCallHome proto should be valid");
        let formatted_log_msg = format!("{cell_log_msg}");
        assert!(
            formatted_log_msg
                .starts_with("spe01 11:12:11: 47.0°C, RSSI  -80 SNR 12.0   LTE-M, 3.85V")
        );

        let status = Status {
            msg: Some(Msg::MiniCallHome(MiniCallHome {
                time: None,
                ..Default::default()
            })),
            ..Default::default()
        };
        let cell_log_msg =
            CellularLogMessage::from_proto(status, MacAddress::default(), "spe01", &tz);
        assert!(cell_log_msg.is_err());
    }
}
