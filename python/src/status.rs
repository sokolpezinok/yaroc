use std::collections::HashSet;

use pyo3::prelude::*;

use chrono::prelude::*;
use geoutils::Location;

use crate::{logs::RssiSnr, punch::SiPunch};

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

#[derive(Clone, Default)]
enum CellularConnectionState {
    #[default]
    Unknown,
    MqttConnected(i8, u32, Option<i16>),
}

#[pyclass]
pub struct NodeInfo {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub rssi_dbm: Option<i8>,
    #[pyo3(get)]
    snr_db: Option<f32>,
    #[pyo3(get)]
    cellid: Option<u32>,
    #[pyo3(get)]
    codes: Vec<u16>,
    #[pyo3(get)]
    last_update: Option<DateTime<FixedOffset>>,
    #[pyo3(get)]
    last_punch: Option<DateTime<FixedOffset>>,
}

#[derive(Default, Clone)]
pub struct CellularRocStatus {
    pub name: String,
    state: CellularConnectionState,
    voltage: Option<f64>,
    codes: HashSet<u16>,
    last_update: Option<DateTime<FixedOffset>>,
    last_punch: Option<DateTime<FixedOffset>>,
}

impl CellularRocStatus {
    pub fn new(name: String) -> Self {
        Self {
            name,
            ..Self::default()
        }
    }

    pub fn disconnect(&mut self) {
        self.state = CellularConnectionState::Unknown;
        self.last_update = Some(Local::now().into());
    }

    pub fn update_voltage(&mut self, voltage: f64) {
        self.voltage = Some(voltage);
    }

    pub fn mqtt_connect_update(&mut self, rssi_dbm: i8, cellid: u32, snr_cb: Option<i16>) {
        self.state = CellularConnectionState::MqttConnected(rssi_dbm, cellid, snr_cb);
        self.last_update = Some(Local::now().into());
    }

    pub fn punch(&mut self, punch: &SiPunch) {
        self.last_punch = Some(punch.time);
        self.codes.insert(punch.code);
    }

    pub fn serialize(&self) -> NodeInfo {
        NodeInfo {
            name: self.name.clone(),
            rssi_dbm: match self.state {
                CellularConnectionState::MqttConnected(rssi_dbm, _, _) => Some(rssi_dbm),
                _ => None,
            },
            snr_db: match self.state {
                CellularConnectionState::MqttConnected(_, _, snr_cb) => {
                    snr_cb.map(|v| f32::from(v) / 10.0)
                }
                _ => None,
            },
            cellid: match self.state {
                CellularConnectionState::MqttConnected(_, cellid, _) if cellid > 0 => Some(cellid),
                _ => None,
            },
            codes: self.codes.iter().copied().collect(),
            last_update: self.last_update,
            last_punch: self.last_punch,
        }
    }
}

#[derive(Default, Clone)]
pub struct MeshtasticRocStatus {
    pub name: String,
    battery: Option<u32>,
    rssi_snr: Option<RssiSnr>,
    pub position: Option<Position>,
    codes: HashSet<u16>,
    last_update: Option<DateTime<FixedOffset>>,
    last_punch: Option<DateTime<FixedOffset>>,
}

impl MeshtasticRocStatus {
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

    pub fn update_rssi_snr(&mut self, rssi_snr: RssiSnr) {
        self.rssi_snr = Some(rssi_snr);
        self.last_update = Some(Local::now().into());
    }

    pub fn clear_rssi_snr(&mut self) {
        self.rssi_snr = None;
        self.last_update = Some(Local::now().into());
    }

    pub fn punch(&mut self, punch: &SiPunch) {
        self.last_punch = Some(punch.time);
        self.codes.insert(punch.code);
    }

    pub fn serialize(&self) -> NodeInfo {
        NodeInfo {
            name: self.name.clone(),
            rssi_dbm: self.rssi_snr.as_ref().map(|x| x.rssi_dbm),
            snr_db: self.rssi_snr.as_ref().map(|x| x.snr),
            cellid: None, // TODO: not supported yet
            codes: self.codes.iter().copied().collect(),
            last_update: self.last_update,
            last_punch: self.last_punch,
        }
    }
}
