use pyo3::{exceptions::PyRuntimeError, prelude::*};
use std::io::Write;

use chrono::prelude::*;
use chrono::{DateTime, Duration};

use crate::status::Position;

#[pyclass]
pub struct MshLogMessage {
    #[pyo3(set)]
    name: String,
    #[pyo3(set)]
    voltage_battery: Option<(f64, u32)>,
    position: Option<Position>,
    #[pyo3(set)]
    dbm_snr: Option<(i16, f32, Option<f32>)>, // TODO: create a struct for this
    #[pyo3(set)]
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
        if let Some((dbm, snr, distance)) = slf.dbm_snr {
            match distance {
                None => write!(&mut buf, ", {}dbm {:.2}SNR", dbm, snr)?,
                Some(meters) => write!(
                    &mut buf,
                    ", {}dBm {:.2}SNR {:.2}km",  // TODO: distance from
                    dbm,
                    snr,
                    meters / 1000.0
                )?,
            }
        }
        String::from_utf8(buf).map_err(|e| PyRuntimeError::new_err(e))
    }
}
