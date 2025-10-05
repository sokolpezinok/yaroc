use chrono::prelude::*;
use chrono::{DateTime, Duration};
use femtopb::{EnumValue, Message};
use std::fmt;

use crate::system_info::{HostInfo, MacAddress};
use yaroc_common::error::Error;
use yaroc_common::proto::status::Msg;
use yaroc_common::proto::{DeviceEvent, Disconnected, EventType, Status};
use yaroc_common::punch::SiPunch;
use yaroc_common::status::{CellNetworkType, MiniCallHome};

#[derive(Debug)]
pub struct SiPunchLog {
    pub punch: SiPunch,
    pub latency: chrono::Duration,
    pub host_info: HostInfo,
}

impl SiPunchLog {
    pub fn from_bytes(
        bytes: &[u8],
        host_info: HostInfo,
        now: DateTime<FixedOffset>,
    ) -> Option<(Self, &[u8])> {
        let (punch, rest) = SiPunch::from_bytes(bytes, now.date_naive(), now.offset())?;
        Some((
            Self {
                latency: now - punch.time,
                punch,
                host_info,
            },
            rest,
        ))
    }
}

impl core::fmt::Display for SiPunchLog {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{} {} punched {} ",
            self.host_info.name, self.punch.card, self.punch.code
        )?;
        write!(f, "at {}", self.punch.time.format("%H:%M:%S.%3f"))?;
        let millis = self.latency.num_milliseconds() as f64 / 1000.0;
        write!(f, ", latency {:4.2}s", millis)
    }
}

#[derive(Clone, Debug)]
pub enum CellularLogMessage {
    Disconnected {
        host_info: HostInfo,
        client: String,
    },
    MCH(MiniCallHomeLog),
    DeviceEvent {
        host_info: HostInfo,
        device_port: String,
        added: bool,
    },
}

impl fmt::Display for CellularLogMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CellularLogMessage::MCH(mch) => write!(f, "{}", mch),
            CellularLogMessage::Disconnected { host_info, client } => {
                write!(f, "{} disconnected client: {client}", host_info.name)
            }
            CellularLogMessage::DeviceEvent {
                host_info,
                device_port,
                added,
            } => {
                let event_type = if *added { "added" } else { "removed" };
                write!(f, "{} {device_port} {event_type}", host_info.name)
            }
        }
    }
}

impl CellularLogMessage {
    pub fn from_proto(
        status: Status,
        host_info: HostInfo,
        now: DateTime<FixedOffset>,
    ) -> crate::Result<Self> {
        match status.msg {
            Some(Msg::Disconnected(Disconnected { client_name, .. })) => {
                Ok(CellularLogMessage::Disconnected {
                    host_info,
                    client: client_name.to_owned(),
                })
            }
            Some(Msg::MiniCallHome(mch)) => {
                let log_message = MiniCallHomeLog::new(host_info, now, mch)?;
                Ok(CellularLogMessage::MCH(log_message))
            }
            Some(Msg::DevEvent(DeviceEvent { port, r#type, .. })) => {
                if let EnumValue::Unknown(_) = r#type {
                    return Err(Error::FormatError.into());
                }
                Ok(CellularLogMessage::DeviceEvent {
                    host_info,
                    device_port: port.to_owned(),
                    added: r#type == EnumValue::Known(EventType::Added),
                })
            }
            _ => Err(Error::FormatError.into()),
        }
    }

    pub fn to_proto(&self) -> Option<Vec<u8>> {
        let status = match self {
            CellularLogMessage::DeviceEvent {
                device_port, added, ..
            } => Some(Status {
                msg: Some(Msg::DevEvent(DeviceEvent {
                    port: device_port,
                    r#type: EnumValue::Known(if *added {
                        EventType::Added
                    } else {
                        EventType::Removed
                    }),
                    ..Default::default()
                })),
                ..Default::default()
            }),
            CellularLogMessage::MCH(mini_call_home_log) => {
                Some(mini_call_home_log.mini_call_home.to_proto())
            }
            _ => None,
        }?;

        let len = status.encoded_len();
        let mut buffer = std::vec![0u8; len];
        if status.encode(&mut buffer.as_mut_slice()).is_ok() {
            Some(buffer)
        } else {
            None
        }
    }

    pub fn mac_address(&self) -> MacAddress {
        match self {
            CellularLogMessage::Disconnected { host_info, .. } => host_info.mac_address,
            CellularLogMessage::MCH(mini_call_home_log) => mini_call_home_log.host_info.mac_address,
            CellularLogMessage::DeviceEvent { host_info, .. } => host_info.mac_address,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MiniCallHomeLog {
    pub mini_call_home: MiniCallHome,
    pub host_info: HostInfo,
    pub latency: Duration,
}

impl MiniCallHomeLog {
    pub fn new(
        host_info: HostInfo,
        now: DateTime<FixedOffset>,
        mch_proto: yaroc_common::proto::MiniCallHome,
    ) -> crate::Result<Self> {
        let mut mch: MiniCallHome = mch_proto.try_into()?;
        mch.timestamp = mch.timestamp.with_timezone(now.offset());
        Ok(Self {
            latency: now - mch.timestamp,
            mini_call_home: mch,
            host_info,
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

        if let Some(signal_info) = &self.mini_call_home.signal_info {
            write!(f, ", RSSI{:5}", signal_info.rssi_dbm)?;
            write!(f, " SNR{:5.1}", f32::from(signal_info.snr_cb) / 10.)?;
            let network_type = match signal_info.network_type {
                CellNetworkType::NbIotEcl0 => "NB ECL0",
                CellNetworkType::NbIotEcl1 => "NB ECL1",
                CellNetworkType::NbIotEcl2 => "NB ECL2",
                CellNetworkType::LteM => "LTE-M",
                CellNetworkType::Umts => "UMTS",
                CellNetworkType::Lte => "LTE",
                _ => "",
            };
            write!(f, " {network_type:>7}")?;
            if let Some(cellid) = &signal_info.cellid {
                write!(f, ", cell {cellid:X}")?;
            }
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
    use femtopb::EnumValue::Known;
    use yaroc_common::{
        proto::{MiniCallHome, Timestamp},
        status::CellSignalInfo,
    };

    #[test]
    fn test_cellular_punch() {
        let correct_punch =
            b"\xff\x02\xd3\x0d\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\x50\xe3\x03";
        let log = SiPunchLog::from_bytes(correct_punch, HostInfo::default(), Local::now().into())
            .unwrap()
            .0;
        assert_eq!(log.punch.card, 1715004);

        let rotated_punch =
            b"\x03\xff\x02\xd3\x0d\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\x50\xe3";
        let log = SiPunchLog::from_bytes(rotated_punch, HostInfo::default(), Local::now().into())
            .unwrap()
            .0;
        assert_eq!(log.punch.card, 1715004);

        let short_punch =
            b"\x03\xff\x02\xd3\x0d\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\x50";
        let log = SiPunchLog::from_bytes(short_punch, HostInfo::default(), Local::now().into());
        assert!(log.is_none());
    }

    #[test]
    fn test_cellular_dbm() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-29T17:40:43+01:00").unwrap();
        let signal_info = Some(CellSignalInfo {
            network_type: CellNetworkType::NbIotEcl0,
            rssi_dbm: -87,
            snr_cb: 70,
            cellid: Some(2580590),
        });
        let log_message = MiniCallHomeLog {
            mini_call_home: yaroc_common::status::MiniCallHome {
                signal_info,
                batt_mv: Some(1260),
                cpu_temperature: Some(51.54),
                timestamp,
                ..Default::default()
            },
            host_info: HostInfo {
                name: "spe01".into(),
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
        let host_info = HostInfo::new("spe01", MacAddress::default());
        let log_message_disconnected = CellularLogMessage::Disconnected {
            host_info: host_info.clone(),
            client: "SIM7020-spe01".to_owned(),
        };
        assert_eq!(
            format!("{log_message_disconnected}"),
            "spe01 disconnected client: SIM7020-spe01"
        );

        let log_message_event = CellularLogMessage::DeviceEvent {
            host_info,
            device_port: "/dev/ttyUSB0".to_owned(),
            added: true,
        };
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
                network_type: Known(yaroc_common::proto::CellNetworkType::LteM),
                signal_dbm: -80,
                signal_snr_cb: 120,
                time: Some(timestamp),
                ..Default::default()
            })),
            ..Default::default()
        };
        let tz = FixedOffset::east_opt(3600).unwrap();
        let now = Local::now().with_timezone(&tz);
        let host_info = HostInfo::new("spe01", MacAddress::default());
        let cell_log_msg = CellularLogMessage::from_proto(status, host_info.clone(), now)
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
        let cell_log_msg = CellularLogMessage::from_proto(status, host_info, now);
        assert!(cell_log_msg.is_err());
    }
}
