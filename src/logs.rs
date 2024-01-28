use pyo3::{exceptions::PyRuntimeError, prelude::*};
use std::io::Write;

use chrono::prelude::*;
use chrono::{DateTime, Duration};

use crate::status::Position;

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
    pub fn new(dbm: i16, snr: f32, distance: Option<(f32, String)>) -> Self {
        Self { dbm, snr, distance }
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
        lat: f64,
        lon: f64,
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
                    ", {dbm}dBm {snr:.2}SNR {:.2}km from {name}", // TODO: distance from
                    meters / 1000.0,
                )?,
            }
        }
        String::from_utf8(buf).map_err(|e| PyRuntimeError::new_err(e))
    }
}
