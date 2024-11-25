use chrono::prelude::*;
use chrono::{DateTime, Duration};
use pyo3::prelude::*;
use std::fmt;

use crate::protobufs::{status::Msg, DeviceEvent, Disconnected, Status};
use crate::protobufs::{EventType, Timestamp};
use crate::status::Position;
use crate::time;

pub enum CellularLogMessage {
    Disconnected(String, String),
    MCH(MiniCallHome),
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
    fn timestamp(time: Timestamp) -> DateTime<FixedOffset> {
        time::datetime_from_timestamp(time.millis_epoch, &Local)
    }

    pub fn from_proto(status: Status, mac_addr: &str, hostname: &str) -> Option<Self> {
        match status.msg {
            Some(Msg::Disconnected(Disconnected { client_name })) => Some(
                CellularLogMessage::Disconnected(hostname.to_owned(), client_name),
            ),
            Some(Msg::MiniCallHome(mch)) => {
                let mut log_message = MiniCallHome::new(
                    hostname,
                    mac_addr,
                    Self::timestamp(mch.time?),
                    Local::now().into(),
                    mch.millivolts,
                );
                if mch.cellid > 0 {
                    log_message.cellid = Some(mch.cellid);
                }
                log_message.rssi_dbm = Some(i16::try_from(mch.signal_dbm).ok()?);
                log_message.snr = Some(i16::try_from(mch.signal_snr).ok()?);
                log_message.temperature = Some(mch.cpu_temperature);
                Some(CellularLogMessage::MCH(log_message))
            }
            Some(Msg::DevEvent(DeviceEvent { port, r#type })) => {
                Some(CellularLogMessage::DeviceEvent(
                    hostname.to_owned(),
                    port,
                    r#type == EventType::Added as i32,
                ))
            }
            None => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
#[pyclass]
pub struct HostInfo {
    pub name: String,
    #[pyo3(get)]
    pub mac_address: String,
}

#[pymethods]
impl HostInfo {
    #[staticmethod]
    fn new(name: String, mac_addr: String) -> Self {
        Self {
            name,
            mac_address: mac_addr,
        }
    }
}

#[pyclass]
pub struct MiniCallHome {
    host_info: HostInfo,
    pub voltage: f32,
    pub rssi_dbm: Option<i16>,
    pub snr: Option<i16>,
    pub cellid: Option<u32>,
    temperature: Option<f32>,
    #[allow(dead_code)]
    cpu_frequency: Option<u32>,
    timestamp: DateTime<FixedOffset>,
    latency: Duration,
}

#[pymethods]
impl MiniCallHome {
    #[new]
    pub fn new(
        name: &str,
        mac_address: &str,
        timestamp: DateTime<FixedOffset>,
        now: DateTime<FixedOffset>,
        millivolts: u32,
    ) -> Self {
        Self {
            host_info: HostInfo {
                name: name.to_owned(),
                mac_address: mac_address.to_owned(),
            },
            timestamp,
            latency: now - timestamp,
            voltage: millivolts as f32 / 1000.0,
            cpu_frequency: None,
            temperature: None,
            rssi_dbm: None,
            snr: None,
            cellid: None,
        }
    }

    pub fn __repr__(&self) -> String {
        format!("{self}")
    }
}

impl fmt::Display for MiniCallHome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let timestamp = self.timestamp.format("%H:%M:%S");
        write!(f, "{} {timestamp}:", self.host_info.name)?;
        if let Some(temperature) = &self.temperature {
            write!(f, " {temperature:.1}°C")?;
        }
        if let Some(rssi_dbm) = &self.rssi_dbm {
            write!(f, ", RSSI{rssi_dbm:5}")?;
            if let Some(snr) = &self.snr {
                write!(f, " SNR{snr:3}")?;
            }
        }
        if let Some(cellid) = &self.cellid {
            write!(f, ", cell {cellid:X}")?;
        }
        write!(f, ", {:.2}V", self.voltage)?;
        let secs = self.latency.num_milliseconds() as f64 / 1000.0;
        write!(f, ", lat. {:4.2}s", secs)
    }
}

#[derive(Clone)]
pub struct RssiSnr {
    pub rssi_dbm: i16,
    pub snr: f32,
    pub distance: Option<(f32, String)>,
}

impl RssiSnr {
    pub fn new(rssi_dbm: i32, snr: f32) -> Option<RssiSnr> {
        match rssi_dbm {
            0 => None,
            rx_rssi => Some(RssiSnr {
                rssi_dbm: rx_rssi as i16,
                snr,
                distance: None,
            }),
        }
    }

    pub fn add_distance(&mut self, dist_m: f32, name: String) {
        self.distance = Some((dist_m, name));
    }
}

pub struct PositionName {
    pub position: Position,
    pub name: String,
}

impl PositionName {
    pub fn new(position: &Position, name: &str) -> Self {
        Self {
            position: position.clone(),
            name: name.to_owned(),
        }
    }
}

#[cfg(test)]
mod test_logs {
    use chrono::{DateTime, Duration};

    use crate::logs::{CellularLogMessage, HostInfo};

    use super::MiniCallHome;

    #[test]
    fn test_cellular_dbm() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-29T17:40:43+01:00").unwrap();
        let log_message = MiniCallHome {
            host_info: HostInfo {
                name: "spe01".to_owned(),
                mac_address: String::new(),
            },
            timestamp,
            latency: Duration::milliseconds(1390),
            voltage: 1.26,
            rssi_dbm: Some(-87),
            snr: Some(7),
            cellid: Some(2580590),
            cpu_frequency: None,
            temperature: Some(51.54),
        };
        assert_eq!(
            format!("{log_message}"),
            "spe01 17:40:43: 51.5°C, RSSI  -87 SNR  7, cell 27606E, 1.26V, lat. 1.39s"
        );
    }

    #[test]
    fn test_cellular_logmessage() {
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
}
