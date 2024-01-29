use meshtastic::protobufs::mesh_packet::PayloadVariant;
use meshtastic::protobufs::{telemetry, Data, ServiceEnvelope, Telemetry};
use meshtastic::protobufs::{MeshPacket, PortNum, Position as PositionProto};
use meshtastic::Message;
use pyo3::exceptions::PyValueError;
use pyo3::{exceptions::PyRuntimeError, prelude::*};
use std::collections::HashMap;
use std::io::Write;

use chrono::prelude::*;
use chrono::{DateTime, Duration};

use crate::status::{MeshtasticRocStatus, Position};

#[pyclass]
pub struct CellularLogMessage {
    name: String,
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
impl CellularLogMessage {
    #[new]
    pub fn new(
        name: String,
        timestamp: DateTime<FixedOffset>,
        now: DateTime<FixedOffset>,
        voltage: f32,
    ) -> Self {
        Self {
            name,
            timestamp,
            latency: now - timestamp,
            voltage,
            cpu_frequency: None,
            temperature: None,
            dbm: None,
            cellid: None,
        }
    }

    pub fn __repr__(slf: PyRef<'_, Self>) -> PyResult<String> {
        let mut buf = Vec::new();
        let timestamp = slf.timestamp.format("%H:%M:%S");
        write!(&mut buf, "{} {timestamp}:", slf.name)?;
        if let Some(temperature) = &slf.temperature {
            write!(&mut buf, " {temperature:.1}°C")?;
        }
        if let Some(dbm) = &slf.dbm {
            write!(&mut buf, ", {dbm}dBm")?;
        }
        if let Some(cellid) = &slf.cellid {
            write!(&mut buf, ", cell {cellid:X}")?;
        }
        write!(&mut buf, ", {:.2}V", slf.voltage)?;
        let millis = slf.latency.num_milliseconds() as f64 / 1000.0;
        write!(&mut buf, ", latency {:4.2}s", millis)?;
        String::from_utf8(buf).map_err(|e| PyRuntimeError::new_err(e))
    }
}

#[pyclass]
#[derive(Clone)]
pub struct DbmSnr {
    dbm: i16,
    snr: f32,
    distance: Option<(f32, String)>,
}

#[pymethods]
impl DbmSnr {
    #[new]
    pub fn with_distance(dbm: i16, snr: f32, distance: Option<(f32, String)>) -> Self {
        Self { dbm, snr, distance }
    }
}

impl DbmSnr {
    pub fn new(rx_rssi: i32, rx_snr: f32) -> Option<DbmSnr> {
        match rx_rssi {
            0 => None,
            rx_rssi => Some(DbmSnr::with_distance(rx_rssi as i16, rx_snr, None)),
        }
    }

    pub fn add_distance(mut self, dist_m: f32, name: String) -> Self {
        self.distance = Some((dist_m, name));
        self
    }
}

#[pyclass]
pub struct MshLogMessage {
    name: String,
    #[pyo3(set)]
    voltage_battery: Option<(f32, u32)>,
    position: Option<Position>,
    #[pyo3(set)]
    dbm_snr: Option<DbmSnr>,
    timestamp: DateTime<FixedOffset>,
    latency: Duration,
}

const TELEMETRY_APP: i32 = PortNum::TelemetryApp as i32;
const POSITION_APP: i32 = PortNum::PositionApp as i32;

#[pymethods]
impl MshLogMessage {
    #[new]
    pub fn new(name: String, timestamp: DateTime<FixedOffset>, now: DateTime<FixedOffset>) -> Self {
        Self {
            name,
            timestamp,
            latency: now - timestamp,
            voltage_battery: None,
            position: None,
            dbm_snr: None,
        }
    }

    pub fn set_position(
        &mut self,
        lat: f32,
        lon: f32,
        elevation: i32,
        timestamp: DateTime<FixedOffset>,
    ) {
        self.position = Some(Position {
            lat,
            lon,
            elevation: elevation as f32,
            timestamp,
        });
    }

    pub fn __repr__(slf: PyRef<'_, Self>) -> PyResult<String> {
        let mut buf = Vec::new();
        let timestamp = slf.timestamp.format("%H:%M:%S");
        write!(&mut buf, "{} {timestamp}:", slf.name)?;
        if let Some((voltage, battery)) = slf.voltage_battery {
            write!(&mut buf, " batt {:.3}V {}%", voltage, battery)?;
        }
        if let Some(Position {
            lat,
            lon,
            elevation,
            ..
        }) = slf.position
        {
            write!(&mut buf, " coords {:.5} {:.5} {}m", lat, lon, elevation)?;
        }
        let millis = slf.latency.num_milliseconds() as f64 / 1000.0;
        write!(&mut buf, ", latency {:4.2}s", millis)?;
        if let Some(DbmSnr { dbm, snr, distance }) = &slf.dbm_snr {
            match distance {
                None => write!(&mut buf, ", {}dbm {:.2}SNR", dbm, snr)?,
                Some((meters, name)) => write!(
                    &mut buf,
                    ", {dbm}dBm {snr:.2}SNR {:.2}km from {name}",
                    meters / 1000.0,
                )?,
            }
        }
        String::from_utf8(buf).map_err(|e| PyRuntimeError::new_err(e))
    }
}

impl MshLogMessage {
    fn timestamp(posix_time: u32) -> DateTime<FixedOffset> {
        Local
            .timestamp_opt(posix_time as i64, 0)
            .unwrap()
            .fixed_offset()
    }

    fn parse_inner(
        data: Data,
        name: &str,
        now: DateTime<FixedOffset>,
        dbm_snr: Option<DbmSnr>,
        recv_position: Option<&Position>,
    ) -> Result<Option<Self>, std::io::Error> {
        // TODO: update dbm_snr based on recv_position
        match data.portnum {
            TELEMETRY_APP => {
                let telemetry = Telemetry::decode(data.payload.as_slice())?;
                let timestamp = Self::timestamp(telemetry.time);
                match telemetry.variant {
                    Some(telemetry::Variant::DeviceMetrics(metrics)) => Ok(Some(Self {
                        name: name.to_owned(),
                        timestamp,
                        latency: now - timestamp,
                        voltage_battery: Some((metrics.voltage, metrics.battery_level)),
                        position: None,
                        dbm_snr,
                    })),
                    _ => Ok(None),
                }
            }
            POSITION_APP => {
                let position = PositionProto::decode(data.payload.as_slice())?;
                let timestamp = Self::timestamp(position.time);
                let position = Position {
                    lat: position.latitude_i as f32 / 10_000_000.,
                    lon: position.longitude_i as f32 / 10_000_000.,
                    elevation: 0.0,
                    timestamp: Self::timestamp(position.time),
                };
                let distance = recv_position
                    .map(|other| position.distance_m(&other))
                    .unwrap()
                    .unwrap();

                Ok(Some(Self {
                    name: name.to_owned(),
                    timestamp,
                    latency: now - timestamp,
                    voltage_battery: None,
                    position: Some(position),
                    dbm_snr: dbm_snr
                        .map(|dbm_snr| dbm_snr.add_distance(distance as f32, "Hahah".to_owned())),
                }))
            }
            _ => Ok(None),
        }
    }

    pub fn from_msh_status(
        payload: &[u8],
        now: DateTime<FixedOffset>,
        dns: &HashMap<String, String>,
        recv_position: Option<&Position>,
    ) -> PyResult<Option<Self>> {
        let service_envelope = ServiceEnvelope::decode(payload)
            .map_err(|e| PyValueError::new_err(format!("Cannot decode proto: {e}")))?;
        match service_envelope.packet {
            Some(MeshPacket {
                payload_variant: Some(PayloadVariant::Decoded(data)),
                from,
                to,
                rx_rssi,
                rx_snr,
                ..
            }) => {
                if data.portnum == POSITION_APP && to == u32::MAX {
                    // Request packets are ignored
                    return Ok(None);
                }
                let name = dns.get(&format!("{:8x}", from)).unwrap();
                Self::parse_inner(data, name, now, DbmSnr::new(rx_rssi, rx_snr), recv_position)
                    .map_err(|_| PyValueError::new_err("Cannot parse inner proto"))
            }
            _ => Err(PyValueError::new_err(
                "Encrypted message, disable encryption in MQTT!",
            )),
        }
    }
}

#[pyclass()]
pub struct MessageHandler {
    dns: HashMap<String, String>,
    meshtastic_statuses: HashMap<String, MeshtasticRocStatus>,
}

#[pymethods()]
impl MessageHandler {
    #[staticmethod]
    pub fn new(dns: HashMap<String, String>) -> Self {
        Self {
            dns,
            meshtastic_statuses: HashMap::new(),
        }
    }

    pub fn msh_status_update(
        &mut self,
        payload: &[u8],
        now: DateTime<FixedOffset>,
        recv_mac_address: &str,
    ) -> PyResult<Option<MshLogMessage>> {
        let recv_position = self.get_position(recv_mac_address);
        let msh_log_message =
            MshLogMessage::from_msh_status(payload, now, &self.dns, recv_position);
        if let Ok(Some(log_message)) = msh_log_message.as_ref() {
            let status = self
                .meshtastic_statuses
                .entry(log_message.name.clone())
                .or_default();
            if let Some(position) = log_message.position.as_ref() {
                status.position = Some(position.clone())
            }
            if let Some(DbmSnr { dbm, .. }) = log_message.dbm_snr.as_ref() {
                status.update_dbm(*dbm);
            }
            if let Some((_, battery)) = log_message.voltage_battery.as_ref() {
                status.update_battery(*battery);
            }
        }
        msh_log_message
    }
}

impl MessageHandler {
    fn get_position(&self, mac_address: &str) -> Option<&Position> {
        let status = self.meshtastic_statuses.get(mac_address)?;
        status.position.as_ref()
    }
}

#[cfg(test)]
mod test_logs {
    use super::MshLogMessage;

    #[test]
    fn test_timestamp() {
        let timestamp = MshLogMessage::timestamp(1706523131)
            .format("%H:%M:%S")
            .to_string();
        assert_eq!("11:12:11", timestamp);
    }
}
