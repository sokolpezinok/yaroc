use pyo3::{exceptions::PyRuntimeError, prelude::*};
use std::time::Duration;

use chrono::prelude::*;
use geoutils::Location;

#[pyclass]
pub struct Position {
    pub lat: f64,
    pub lon: f64,
    pub elevation: f32,
    pub timestamp: chrono::DateTime<FixedOffset>,
}

#[pymethods]
impl Position {
    #[staticmethod]
    pub fn new(lat: f64, lon: f64, timestamp: DateTime<FixedOffset>) -> Self {
        Self {
            lat,
            lon,
            elevation: 0.0,
            timestamp,
        }
    }

    pub fn distance_m(&self, other: &Position) -> PyResult<f64> {
        let me = Location::new(self.lat, self.lon);
        let other = Location::new(other.lat, other.lon);
        Ok(me
            .distance_to(&other)
            .map_err(|e| PyRuntimeError::new_err(e))?
            .meters())
    }
}

struct LogMessage {
    name: String,
    timestamp: chrono::DateTime<Local>,
    latency: Duration,
    position: Option<Position>,
    dbm: Option<i16>,
    cell: Option<u32>,
    snr: Option<f32>,
}
