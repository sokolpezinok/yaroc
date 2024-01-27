use std::collections::HashSet;

use pyo3::{exceptions::PyRuntimeError, prelude::*};

use chrono::prelude::*;
use geoutils::Location;

use crate::punch::SiPunch;

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

enum CellularConnectionState {
    Unknown,
    Unregistered,
    Registered(i16, u32),
    MqttConnected(i16, u32),
}

#[pyclass]
struct CellularRocStatus {
    state: CellularConnectionState,
    voltage: Option<f64>,
    codes: HashSet<u16>,
    last_update: Option<DateTime<FixedOffset>>,
    last_punch: Option<DateTime<FixedOffset>>,
}

#[pymethods]
impl CellularRocStatus {
    pub fn disconnect(&mut self) {
        self.state = CellularConnectionState::Unknown;
        self.last_update = Some(Local::now().into());
    }

    pub fn update_voltage(&mut self, voltage: f64) {
        self.voltage = Some(voltage);
    }

    pub fn mqtt_connect_update(&mut self, dbm: i16, cellid: u32) {
        self.state = CellularConnectionState::MqttConnected(dbm, cellid);
        self.last_update = Some(Local::now().into());
    }

    pub fn punch(&mut self, punch: &SiPunch) {
        self.last_punch = Some(punch.time);
        self.codes.insert(punch.code);
    }
}
