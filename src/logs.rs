use chrono::prelude::*;
use chrono::{DateTime, Duration};
use meshtastic::protobufs::mesh_packet::PayloadVariant;
use meshtastic::protobufs::{telemetry, Data, ServiceEnvelope, Telemetry};
use meshtastic::protobufs::{MeshPacket, PortNum, Position as PositionProto};
use meshtastic::Message as MeshtaticMessage;
use pyo3::prelude::*;
use std::collections::HashMap;
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
                    mch.volts,
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

#[derive(Clone, Debug, PartialEq)]
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
        voltage: f32,
    ) -> Self {
        Self {
            host_info: HostInfo {
                name: name.to_owned(),
                mac_address: mac_address.to_owned(),
            },
            timestamp,
            latency: now - timestamp,
            voltage,
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
    distance: Option<(f32, String)>,
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

#[pyclass]
pub struct MshLogMessage {
    pub host_info: HostInfo,
    pub voltage_battery: Option<(f32, u32)>,
    pub position: Option<Position>,
    pub rssi_snr: Option<RssiSnr>,
    timestamp: DateTime<FixedOffset>,
    latency: Duration,
}

const TELEMETRY_APP: i32 = PortNum::TelemetryApp as i32;
const POSITION_APP: i32 = PortNum::PositionApp as i32;

#[pymethods]
impl MshLogMessage {
    pub fn __repr__(&self) -> String {
        format!("{}", self)
    }
}

impl fmt::Display for MshLogMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let timestamp = self.timestamp.format("%H:%M:%S");
        write!(f, "{} {timestamp}:", self.host_info.name)?;
        if let Some((voltage, battery)) = self.voltage_battery {
            write!(f, " batt {:.3}V {}%", voltage, battery)?;
        }
        if let Some(Position {
            lat,
            lon,
            elevation,
            ..
        }) = self.position
        {
            write!(f, " coords {:.5} {:.5} {}m", lat, lon, elevation)?;
        }
        let millis = self.latency.num_milliseconds() as f64 / 1000.0;
        write!(f, ", latency {:4.2}s", millis)?;
        if let Some(RssiSnr {
            rssi_dbm,
            snr,
            distance,
        }) = &self.rssi_snr
        {
            match distance {
                None => write!(f, ", {}dBm {:.2}SNR", rssi_dbm, snr)?,
                Some((meters, name)) => write!(
                    f,
                    ", {rssi_dbm}dBm {snr:.2}SNR, {:.2}km from {name}",
                    meters / 1000.0,
                )?,
            }
        };
        Ok(())
    }
}

pub struct PositionName {
    position: Position,
    name: String,
}

impl PositionName {
    pub fn new(position: &Position, name: &str) -> Self {
        Self {
            position: position.clone(),
            name: name.to_owned(),
        }
    }
}

impl MshLogMessage {
    pub fn timestamp(posix_time: u32) -> DateTime<FixedOffset> {
        time::datetime_from_timestamp(u64::from(posix_time) * 1000, &Local)
    }

    fn parse_inner(
        data: Data,
        host_info: HostInfo,
        now: DateTime<FixedOffset>,
        mut rssi_snr: Option<RssiSnr>,
        recv_position: Option<PositionName>,
    ) -> Result<Option<Self>, std::io::Error> {
        match data.portnum {
            TELEMETRY_APP => {
                let telemetry = Telemetry::decode(data.payload.as_slice())?;
                let timestamp = Self::timestamp(telemetry.time);
                match telemetry.variant {
                    Some(telemetry::Variant::DeviceMetrics(metrics)) => Ok(Some(Self {
                        host_info,
                        timestamp,
                        latency: now - timestamp,
                        voltage_battery: Some((metrics.voltage, metrics.battery_level)),
                        position: None,
                        rssi_snr,
                    })),
                    _ => Ok(None),
                }
            }
            POSITION_APP => {
                let position = PositionProto::decode(data.payload.as_slice())?;
                if position.latitude_i == 0 && position.longitude_i == 0 {
                    return Ok(None);
                }
                let timestamp = Self::timestamp(position.time);
                let position = Position {
                    lat: position.latitude_i as f32 / 10_000_000.,
                    lon: position.longitude_i as f32 / 10_000_000.,
                    elevation: position.altitude,
                    timestamp: Self::timestamp(position.time),
                };
                let distance = recv_position
                    .as_ref()
                    .map(|other| position.distance_m(&other.position));
                if let Some(Ok(distance)) = distance {
                    rssi_snr.as_mut().map(|rssi_snr| {
                        rssi_snr.add_distance(
                            distance as f32,
                            recv_position.map(|x| x.name).unwrap_or_default(),
                        )
                    });
                }

                Ok(Some(Self {
                    host_info,
                    timestamp,
                    latency: now - timestamp,
                    voltage_battery: None,
                    position: Some(position),
                    rssi_snr,
                }))
            }
            _ => Ok(None),
        }
    }

    pub fn from_msh_status(
        payload: &[u8],
        now: DateTime<FixedOffset>,
        dns: &HashMap<String, String>,
        recv_position: Option<PositionName>,
    ) -> Result<Option<Self>, std::io::Error> {
        let service_envelope = ServiceEnvelope::decode(payload)?;
        match service_envelope.packet {
            Some(MeshPacket {
                payload_variant: Some(PayloadVariant::Decoded(data)),
                from,
                rx_rssi,
                rx_snr,
                ..
            }) => {
                let mac_address = format!("{:8x}", from);
                let name = dns
                    .get(&mac_address)
                    .map(|x| x.as_str())
                    .unwrap_or("Unknown");
                Self::parse_inner(
                    data,
                    HostInfo {
                        name: name.to_owned(),
                        mac_address,
                    },
                    now,
                    RssiSnr::new(rx_rssi, rx_snr),
                    recv_position,
                )
            }
            Some(MeshPacket {
                payload_variant: Some(PayloadVariant::Encrypted(_)),
                ..
            }) => Err(
                prost::DecodeError::new("Encrypted message, disable encryption in MQTT!").into(),
            ),
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod test_logs {
    use chrono::{DateTime, Duration};

    use crate::{
        logs::{CellularLogMessage, HostInfo, RssiSnr},
        status::Position,
    };

    use super::{MiniCallHome, MshLogMessage};

    #[test]
    fn test_volt_batt() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-29T21:34:49+01:00").unwrap();
        let log_message = MshLogMessage {
            host_info: HostInfo {
                name: "spr01".to_owned(),
                mac_address: String::new(),
            },
            timestamp,
            latency: Duration::milliseconds(1230),
            voltage_battery: Some((4.012, 82)),
            position: None,
            rssi_snr: None,
        };
        assert_eq!(
            format!("{log_message}"),
            "spr01 21:34:49: batt 4.012V 82%, latency 1.23s"
        );
    }

    #[test]
    fn test_position() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-29T13:15:25+01:00").unwrap();
        let log_message = MshLogMessage {
            host_info: HostInfo {
                name: "spr01".to_owned(),
                mac_address: String::new(),
            },
            timestamp,
            latency: Duration::milliseconds(1230),
            position: Some(Position {
                lat: 48.29633,
                lon: 17.26675,
                elevation: 170,
                timestamp,
            }),
            voltage_battery: None,
            rssi_snr: None,
        };
        assert_eq!(
            format!("{log_message}"),
            "spr01 13:15:25: coords 48.29633 17.26675 170m, latency 1.23s",
        );
    }

    #[test]
    fn test_position_dbm() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-29T13:15:25+01:00").unwrap();
        let log_message = MshLogMessage {
            host_info: HostInfo {
                name: "spr01".to_owned(),
                mac_address: String::new(),
            },
            timestamp,
            latency: Duration::milliseconds(1230),
            position: Some(Position {
                lat: 48.29633,
                lon: 17.26675,
                elevation: 170,
                timestamp,
            }),
            voltage_battery: None,
            rssi_snr: Some(RssiSnr {
                rssi_dbm: -80,
                snr: 4.25,
                distance: Some((813., "spr02".to_string())),
            }),
        };
        assert_eq!(
            format!("{log_message}"),
            "spr01 13:15:25: coords 48.29633 17.26675 170m, latency 1.23s, -80dBm 4.25SNR, 0.81km \
            from spr02"
        );
    }

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
