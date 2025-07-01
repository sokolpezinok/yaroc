extern crate std;

use std::collections::HashSet;
use std::string::String;
use std::vec::Vec;

use chrono::prelude::*;

#[cfg(feature = "receive")]
use crate::meshtastic::RssiSnr;
use crate::punch::SiPunch;
use crate::status::CellSignalInfo;

pub enum SignalInfo {
    Uknown,
    Cell(CellSignalInfo),
    #[cfg(feature = "receive")]
    Meshtastic(RssiSnr),
}

pub struct NodeInfo {
    pub name: String,
    pub signal_info: SignalInfo,
    pub codes: Vec<u16>,
    pub last_update: Option<DateTime<FixedOffset>>,
    pub last_punch: Option<DateTime<FixedOffset>>,
}

#[derive(Default, Clone)]
pub struct CellularRocStatus {
    pub name: String,
    state: Option<CellSignalInfo>,
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
        self.state = None;
        self.last_update = Some(Local::now().into());
    }

    pub fn update_voltage(&mut self, voltage: f64) {
        self.voltage = Some(voltage);
    }

    pub fn mqtt_connect_update(&mut self, signal_info: CellSignalInfo) {
        self.state = Some(signal_info);
        self.last_update = Some(Local::now().into());
    }

    pub fn punch(&mut self, punch: &SiPunch) {
        self.last_punch = Some(punch.time);
        self.codes.insert(punch.code);
    }

    pub fn serialize(&self) -> NodeInfo {
        let signal_info = match self.state {
            Some(signal_info) => SignalInfo::Cell(signal_info),
            None => SignalInfo::Uknown,
        };

        NodeInfo {
            name: self.name.clone(),
            signal_info,
            codes: self.codes.iter().copied().collect(),
            last_update: self.last_update,
            last_punch: self.last_punch,
        }
    }
}

#[cfg(feature = "receive")]
#[derive(Default, Clone)]
pub struct MeshtasticRocStatus {
    pub name: String,
    battery: Option<u32>,
    pub rssi_snr: Option<RssiSnr>,
    pub position: Option<crate::status::Position>,
    codes: HashSet<u16>,
    last_update: Option<DateTime<FixedOffset>>,
    last_punch: Option<DateTime<FixedOffset>>,
}

#[cfg(feature = "receive")]
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
        let signal_info = match &self.rssi_snr {
            Some(rssi_snr) => SignalInfo::Meshtastic(rssi_snr.clone()),
            None => SignalInfo::Uknown,
        };
        NodeInfo {
            name: self.name.clone(),
            signal_info,
            codes: self.codes.iter().copied().collect(),
            last_update: self.last_update,
            last_punch: self.last_punch,
        }
    }
}
