use log::error;
use log::info;
use prost::Message;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use std::collections::HashMap;

use chrono::prelude::*;
use chrono::DateTime;

use crate::logs::{CellularLogMessage, HostInfo, MshLogMessage, PositionName};
use crate::protobufs::{Punches, Status};
use crate::punch::SiPunch;
use crate::status::{CellularRocStatus, MeshtasticRocStatus, NodeInfo};

#[pyclass()]
pub struct MessageHandler {
    dns: HashMap<String, String>,
    cellular_statuses: HashMap<String, CellularRocStatus>,
    meshtastic_statuses: HashMap<String, MeshtasticRocStatus>,
    meshtastic_override_mac: Option<String>,
}

#[pymethods]
impl MessageHandler {
    #[staticmethod]
    pub fn new(dns: HashMap<String, String>, meshtastic_override_mac: Option<String>) -> Self {
        Self {
            dns,
            meshtastic_statuses: HashMap::new(),
            cellular_statuses: HashMap::new(),
            meshtastic_override_mac,
        }
    }

    fn resolve(&self, mac_addr: &str) -> &str {
        self.dns
            .get(mac_addr)
            .map(|x| x.as_str())
            .unwrap_or("Unknown")
    }

    pub fn msh_serial_msg(&mut self, payload: &[u8]) -> PyResult<SiPunch> {
        let mut punch = SiPunch::from_msh_serial(payload)?;
        let mac_addr = &punch.host_info.mac_address;
        let name = self.resolve(mac_addr).to_owned();
        punch.host_info.name = name.to_owned();
        let status = self
            .meshtastic_statuses
            .entry(mac_addr.to_owned())
            .or_insert(MeshtasticRocStatus::new(name));
        status.punch(&punch);
        if let Some(mac_addr) = self.meshtastic_override_mac.as_ref() {
            punch.host_info.mac_address = mac_addr.clone();
        }

        Ok(punch)
    }

    pub fn msh_status_update(
        &mut self,
        payload: &[u8],
        now: DateTime<FixedOffset>,
        recv_mac_address: Option<String>,
    ) {
        let recv_position =
            recv_mac_address.and_then(|mac_addr| self.get_position_name(mac_addr.as_ref()));
        let msh_log_message =
            MshLogMessage::from_msh_status(payload, now, &self.dns, recv_position);
        match msh_log_message {
            Err(err) => {
                error!("Failed to parse msh status proto: {}", err);
            }
            Ok(Some(log_message)) => {
                info!("{}", log_message);
                let status = self
                    .meshtastic_statuses
                    .entry(log_message.host_info.mac_address.clone())
                    .or_insert(MeshtasticRocStatus::new(log_message.host_info.name.clone()));
                if let Some(position) = log_message.position.as_ref() {
                    status.position = Some(position.clone())
                }
                if let Some(rssi_snr) = log_message.rssi_snr.as_ref() {
                    status.update_dbm(rssi_snr.clone());
                }
                if let Some((_, battery)) = log_message.voltage_battery.as_ref() {
                    status.update_battery(*battery);
                }
            }
            _ => {}
        }
    }

    pub fn node_infos(&self) -> Vec<NodeInfo> {
        let mut res: Vec<_> = self
            .meshtastic_statuses
            .values()
            .map(|status| status.serialize())
            .chain(
                self.cellular_statuses
                    .values()
                    .map(|status| status.serialize()),
            )
            .collect();
        res.sort_by(|a, b| a.name.cmp(&b.name));
        res
    }

    pub fn punches(&mut self, payload: &[u8], mac_addr: &str) -> PyResult<Vec<SiPunch>> {
        let punches =
            Punches::decode(payload).map_err(|_| PyValueError::new_err("Failed to parse proto"))?;
        let host_info: HostInfo = HostInfo {
            name: self.resolve(mac_addr).to_owned(),
            mac_address: mac_addr.to_owned(),
        };
        let status = self.get_cellular_status(mac_addr);
        let mut result = Vec::new();
        for punch in punches.punches {
            let si_punch = SiPunch::from_proto(punch, &host_info);
            if let Ok(si_punch) = si_punch {
                status.punch(&si_punch);
                result.push(si_punch);
            }
        }
        Ok(result)
    }

    pub fn status_update(&mut self, payload: &[u8], mac_addr: &str) -> PyResult<()> {
        let status_proto =
            Status::decode(payload).map_err(|e| PyValueError::new_err(format!("{e}")))?;
        let log_message =
            CellularLogMessage::from_proto(status_proto, mac_addr, &self.resolve(&mac_addr))
                .ok_or(PyValueError::new_err(
                    "Variants other than MiniCallHome are unimplemented",
                ))?;
        info!("{}", log_message);

        let status = self.get_cellular_status(mac_addr);
        match log_message {
            CellularLogMessage::MCH(mch) => {
                if let Some(rssi_dbm) = mch.rssi_dbm {
                    status.mqtt_connect_update(
                        rssi_dbm as i16,
                        mch.cellid.unwrap_or_default(),
                        mch.snr,
                    );
                }
                status.update_voltage(f64::from(mch.voltage));
            }
            CellularLogMessage::Disconnected(..) => {
                status.disconnect();
            }
            _ => {}
        }
        Ok(())
    }
}

impl MessageHandler {
    fn get_position_name(&self, mac_address: &str) -> Option<PositionName> {
        let status = self.meshtastic_statuses.get(mac_address)?;
        status
            .position
            .as_ref()
            .map(|position| PositionName::new(position, &status.name))
    }

    fn get_cellular_status(&mut self, mac_addr: &str) -> &mut CellularRocStatus {
        let name = self.resolve(mac_addr).to_owned();
        self.cellular_statuses
            .entry(mac_addr.to_owned())
            .or_insert(CellularRocStatus::new(name))
    }
}
