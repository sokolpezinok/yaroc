use meshtastic::protobufs::mesh_packet::PayloadVariant;
use meshtastic::protobufs::{telemetry, Data, ServiceEnvelope, Telemetry};
use meshtastic::protobufs::{MeshPacket, PortNum, Position as PositionProto};
use meshtastic::Message as MeshtaticMessage;
use prost::Message;
use prost_wkt_types::Timestamp;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use std::collections::HashMap;
use std::fmt;

use chrono::prelude::*;
use chrono::{DateTime, Duration};

use crate::protobufs::{status::Msg, Disconnected, Punches, Status};
use crate::punch::SiPunch;
use crate::status::{CellularRocStatus, HostInfo, MeshtasticRocStatus, NodeInfo, Position};

fn timestamp<T: TimeZone>(posix_time: i64, nanos: u32, tz: &T) -> DateTime<FixedOffset> {
    tz.timestamp_opt(posix_time, nanos).unwrap().fixed_offset()
}

pub enum CellularLogMessage {
    Disconnected(String),
    MCH(MiniCallHome),
}

impl CellularLogMessage {
    fn timestamp(time: Timestamp) -> DateTime<FixedOffset> {
        timestamp(time.seconds, time.nanos as u32, &Local)
    }

    pub fn from_proto(status: Status, mac_addr: &str, name: &str) -> Option<Self> {
        match status.msg {
            Some(Msg::Disconnected(Disconnected { client_name })) => {
                Some(CellularLogMessage::Disconnected(client_name))
            }
            Some(Msg::MiniCallHome(mch)) => {
                let mut log_message = MiniCallHome::new(
                    name,
                    mac_addr,
                    Self::timestamp(mch.time.unwrap()),
                    Local::now().into(),
                    mch.volts,
                );
                if mch.cellid > 0 {
                    log_message.cellid = Some(mch.cellid);
                }
                log_message.dbm = Some(mch.signal_dbm);
                log_message.temperature = Some(mch.cpu_temperature);
                Some(CellularLogMessage::MCH(log_message))
            }
            Some(Msg::DevEvent(_)) => None,
            None => None,
        }
    }
}

#[pyclass]
pub struct MiniCallHome {
    host_info: HostInfo,
    voltage: f32,
    #[pyo3(set)]
    dbm: Option<i32>,
    #[pyo3(set)]
    cellid: Option<u32>,
    #[pyo3(set)]
    temperature: Option<f32>,
    #[pyo3(set)]
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
            dbm: None,
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
        if let Some(dbm) = &self.dbm {
            write!(f, ", {dbm}dBm")?;
        }
        if let Some(cellid) = &self.cellid {
            write!(f, ", cell {cellid:X}")?;
        }
        write!(f, ", {:.2}V", self.voltage)?;
        let millis = self.latency.num_milliseconds() as f64 / 1000.0;
        write!(f, ", latency {:4.2}s", millis)
    }
}

#[derive(Clone)]
pub struct RssiSnr {
    rssi_dbm: i16,
    snr: f32,
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
    host_info: HostInfo,
    voltage_battery: Option<(f32, u32)>,
    position: Option<Position>,
    rssi_snr: Option<RssiSnr>,
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

impl MshLogMessage {
    pub fn timestamp(posix_time: u32) -> DateTime<FixedOffset> {
        timestamp(i64::from(posix_time), 0, &Local)
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
                        rssi_snr.add_distance(distance as f32, recv_position.unwrap().name)
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
    ) -> PyResult<Option<Self>> {
        let service_envelope = ServiceEnvelope::decode(payload)
            .map_err(|e| PyValueError::new_err(format!("Cannot decode proto: {e}")))?;
        match service_envelope.packet {
            Some(MeshPacket {
                payload_variant: Some(PayloadVariant::Decoded(data)),
                from,
                rx_rssi,
                rx_snr,
                ..
            }) => {
                let mac_address = format!("{:8x}", from);
                let name = dns.get(&mac_address).unwrap();
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
                .map_err(|_| PyValueError::new_err("Cannot parse inner proto"))
            }
            Some(MeshPacket {
                payload_variant: Some(PayloadVariant::Encrypted(_)),
                ..
            }) => Err(PyValueError::new_err(
                "Encrypted message, disable encryption in MQTT!",
            )),
            _ => Ok(None),
        }
    }
}

#[pyclass()]
pub struct MessageHandler {
    dns: HashMap<String, String>,
    cellular_statuses: HashMap<String, CellularRocStatus>,
    meshtastic_statuses: HashMap<String, MeshtasticRocStatus>,
}

#[pymethods]
impl MessageHandler {
    #[staticmethod]
    pub fn new(dns: HashMap<String, String>) -> Self {
        Self {
            dns,
            meshtastic_statuses: HashMap::new(),
            cellular_statuses: HashMap::new(),
        }
    }

    pub fn msh_serial_update(&mut self, payload: &[u8]) -> PyResult<SiPunch> {
        let punch = SiPunch::from_msh_serial(payload)?;
        let status = self
            .meshtastic_statuses
            .entry(punch.mac_addr.clone())
            .or_insert(MeshtasticRocStatus::new(
                self.dns.get(&punch.mac_addr).unwrap().to_owned(),
            ));
        status.punch(&punch);

        Ok(punch)
    }

    pub fn msh_status_update(
        &mut self,
        payload: &[u8],
        now: DateTime<FixedOffset>,
        recv_mac_address: &str,
    ) -> PyResult<Option<MshLogMessage>> {
        let recv_position = self.get_position_name(recv_mac_address);
        let msh_log_message =
            MshLogMessage::from_msh_status(payload, now, &self.dns, recv_position);
        if let Ok(Some(log_message)) = msh_log_message.as_ref() {
            let status = self
                .meshtastic_statuses
                .entry(log_message.host_info.mac_address.clone())
                .or_insert(MeshtasticRocStatus::new(log_message.host_info.name.clone()));
            if let Some(position) = log_message.position.as_ref() {
                status.position = Some(position.clone())
            }
            if let Some(RssiSnr { rssi_dbm, .. }) = log_message.rssi_snr.as_ref() {
                status.update_dbm(*rssi_dbm);
            }
            if let Some((_, battery)) = log_message.voltage_battery.as_ref() {
                status.update_battery(*battery);
            }
        }
        msh_log_message
    }

    pub fn node_infos(&self) -> Vec<NodeInfo> {
        self.meshtastic_statuses
            .values()
            .map(|status| status.serialize())
            .chain(
                self.cellular_statuses
                    .values()
                    .map(|status| status.serialize()),
            )
            .collect()
    }

    pub fn punches(&mut self, payload: &[u8], mac_addr: &str) -> PyResult<Vec<SiPunch>> {
        let punches =
            Punches::decode(payload).map_err(|_| PyValueError::new_err("Failed to parse proto"))?;
        let status = self.get_cellular_status(mac_addr);
        let mut result = Vec::new();
        for punch in punches.punches {
            let si_punch = SiPunch::from_proto(punch, mac_addr);
            if let Ok(si_punch) = si_punch {
                status.punch(&si_punch);
                result.push(si_punch);
            }
        }
        Ok(result)
    }

    pub fn status_update(&mut self, payload: &[u8], mac_addr: &str) -> PyResult<MiniCallHome> {
        let status_proto =
            Status::decode(payload).map_err(|e| PyValueError::new_err(format!("{e}")))?;
        let log_message =
            CellularLogMessage::from_proto(status_proto, mac_addr, self.dns.get(mac_addr).unwrap())
                .ok_or(PyValueError::new_err(
                    "Variants other than MiniCallHome are unimplemented",
                ))?; // TODO

        let status = self.get_cellular_status(mac_addr);
        match log_message {
            CellularLogMessage::MCH(mch) => {
                if let Some(dbm) = mch.dbm {
                    status.mqtt_connect_update(dbm as i16, mch.cellid.unwrap_or_default());
                }
                status.update_voltage(f64::from(mch.voltage));
                Ok(mch)
            }
            CellularLogMessage::Disconnected(_) => {
                status.disconnect();
                Err(PyValueError::new_err(
                    "Variants other than MiniCallHome are unimplemented",
                ))
            }
        }
    }
}

impl MessageHandler {
    fn get_position_name(&self, mac_address: &str) -> Option<PositionName> {
        let status = self.meshtastic_statuses.get(mac_address)?;
        status.position.as_ref().map(|position| PositionName {
            position: position.clone(),
            name: status.name.clone(),
        })
    }

    fn get_cellular_status(&mut self, mac_addr: &str) -> &mut CellularRocStatus {
        self.cellular_statuses
            .entry(mac_addr.to_owned())
            .or_insert(CellularRocStatus::new(
                self.dns.get(mac_addr).unwrap().to_owned(),
            ))
    }
}

pub struct PositionName {
    position: Position,
    name: String,
}

#[cfg(test)]
mod test_logs {
    use chrono::{DateTime, Duration, FixedOffset};

    use crate::{
        message_handler::{timestamp, RssiSnr},
        status::{HostInfo, Position},
    };

    use super::{MiniCallHome, MshLogMessage};

    #[test]
    fn test_timestamp() {
        let tz = FixedOffset::east_opt(3600).unwrap();
        let timestamp = timestamp(1706523131, 0, &tz).format("%H:%M:%S").to_string();
        assert_eq!("11:12:11", timestamp);
    }

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
            dbm: Some(-87),
            cellid: Some(2580590),
            cpu_frequency: None,
            temperature: Some(51.54),
        };
        assert_eq!(
            format!("{log_message}"),
            "spe01 17:40:43: 51.5°C, -87dBm, cell 27606E, 1.26V, latency 1.39s"
        );
    }
}
