use std::collections::HashSet;

use pyo3::prelude::*;

use chrono::prelude::*;
use geoutils::Location;

use crate::punch::SiPunch;

#[derive(Clone, Debug)]
pub struct HostInfo {
    pub name: String,
    pub mac_address: String,
}

#[derive(Clone, Debug)]
pub struct Position {
    pub lat: f32,
    pub lon: f32,
    pub elevation: i32,
    pub timestamp: chrono::DateTime<FixedOffset>,
}

impl Position {
    pub fn new(lat: f32, lon: f32, timestamp: DateTime<FixedOffset>) -> Self {
        Self {
            lat,
            lon,
            elevation: 0,
            timestamp,
        }
    }

    pub fn distance_m(&self, other: &Position) -> Result<f64, String> {
        let me = Location::new(self.lat, self.lon);
        let other = Location::new(other.lat, other.lon);
        Ok(me.distance_to(&other)?.meters())
    }
}

enum CellularConnectionState {
    Unknown,
    Unregistered,
    Registered(i16, u32),
    MqttConnected(i16, u32),
}

impl Default for CellularConnectionState {
    fn default() -> CellularConnectionState {
        CellularConnectionState::Unknown
    }
}

#[derive(Default)]
#[pyclass]
pub struct CellularRocStatus {
    state: CellularConnectionState,
    voltage: Option<f64>,
    codes: HashSet<u16>,
    last_update: Option<DateTime<FixedOffset>>,
    last_punch: Option<DateTime<FixedOffset>>,
}

#[pyclass]
pub struct NodeInfo {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    dbm: Option<i16>,
    #[pyo3(get)]
    codes: Vec<u16>,
    #[pyo3(get)]
    last_update: Option<DateTime<FixedOffset>>,
    #[pyo3(get)]
    last_punch: Option<DateTime<FixedOffset>>,
}

#[pymethods]
impl CellularRocStatus {
    #[staticmethod]
    pub fn new() -> Self {
        Self::default()
    }

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

    pub fn serialize(&self, name: &str) -> NodeInfo {
        NodeInfo {
            name: name.to_owned(),
            dbm: match self.state {
                CellularConnectionState::MqttConnected(dbm, _) => Some(dbm),
                CellularConnectionState::Registered(dbm, _) => Some(dbm),
                _ => None,
            },
            codes: self.codes.iter().map(|x| *x).collect(),
            last_update: self.last_update,
            last_punch: self.last_punch,
        }
    }
}

#[pyclass]
#[derive(Default)]
pub struct MeshtasticRocStatus {
    pub name: String,
    battery: Option<u32>,
    dbm: Option<i16>,
    pub position: Option<Position>,
    codes: HashSet<u16>,
    last_update: Option<DateTime<FixedOffset>>,
    last_punch: Option<DateTime<FixedOffset>>,
}

#[pymethods]
impl MeshtasticRocStatus {
    #[staticmethod]
    pub fn new(name: String) -> Self {
        Self {
            name,
            ..Default::default()
        }
    }

    pub fn update_battery(&mut self, battery: u32) {
        self.battery = Some(battery);
        self.last_update = Some(Local::now().into());
    }

    pub fn update_dbm(&mut self, dbm: i16) {
        self.dbm = Some(dbm);
        self.last_update = Some(Local::now().into());
    }

    pub fn punch(&mut self, punch: &SiPunch) {
        self.last_punch = Some(punch.time);
        self.codes.insert(punch.code);
    }

    pub fn serialize(&self) -> NodeInfo {
        NodeInfo {
            name: self.name.clone(),
            dbm: self.dbm,
            codes: self.codes.iter().map(|x| *x).collect(),
            last_update: self.last_update,
            last_punch: self.last_punch,
        }
    }
}
