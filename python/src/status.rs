use std::collections::HashSet;

use pyo3::{exceptions::PyValueError, prelude::*};

use chrono::prelude::*;

use crate::punch::SiPunch;
use yaroc_common::status::{HostInfo, MacAddress, Position, RssiSnr};

#[derive(Clone, Debug, Default, PartialEq)]
#[pyclass(name = "HostInfo")]
pub struct HostInfoPy {
    host_info: HostInfo,
}

#[pymethods]
impl HostInfoPy {
    #[staticmethod]
    pub fn new(name: &str, mac_addr: &str) -> PyResult<Self> {
        let mac_address = MacAddress::try_from(mac_addr)
            .map_err(|_| PyValueError::new_err("MAC address malformatted"))?;
        Ok(HostInfo::new(name, mac_address)
            .map_err(|_| PyValueError::new_err("Name too long"))?
            .into())
    }

    #[getter(mac_address)]
    pub fn mac_address_str(&self) -> String {
        self.host_info.mac_address.to_string()
    }
}

impl HostInfoPy {
    pub fn name(&self) -> &str {
        &self.host_info.name
    }

    pub fn mac_address(&self) -> &MacAddress {
        &self.host_info.mac_address
    }
}

impl From<HostInfo> for HostInfoPy {
    fn from(value: HostInfo) -> Self {
        Self { host_info: value }
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
    pub rssi_dbm: Option<i16>,
    #[pyo3(get)]
    pub snr_db: Option<f32>,
    #[pyo3(get)]
    cellid: Option<u32>,
    #[pyo3(get)]
    codes: Vec<u16>,
    #[pyo3(get)]
    last_update: Option<DateTime<FixedOffset>>,
    #[pyo3(get)]
    pub last_punch: Option<DateTime<FixedOffset>>,
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
                CellularConnectionState::MqttConnected(rssi_dbm, _, _) => Some(i16::from(rssi_dbm)),
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
