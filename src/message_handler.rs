use log::error;
use log::info;
use meshtastic::protobufs::mesh_packet::PayloadVariant;
use meshtastic::protobufs::Data;
use meshtastic::protobufs::PortNum;
use meshtastic::protobufs::ServiceEnvelope;
use meshtastic::Message as MeshtasticMessage;
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

    pub fn msh_serial_msg(&mut self, payload: &[u8]) -> PyResult<Vec<SiPunch>> {
        let service_envelope =
            ServiceEnvelope::decode(payload).map_err(|e| std::io::Error::from(e))?;
        let packet = service_envelope
            .packet
            .ok_or(PyValueError::new_err("Missing packet in ServiceEnvelope"))?;
        let mac_addr = format!("{:8x}", packet.from);
        const SERIAL_APP: i32 = PortNum::SerialApp as i32;
        let now = Local::now().fixed_offset();
        let host_info = HostInfo {
            name: self.resolve(&mac_addr).to_owned(),
            mac_address: mac_addr.clone(),
        };
        let punches = match packet.payload_variant {
            Some(PayloadVariant::Decoded(Data {
                portnum: SERIAL_APP,
                payload,
                ..
            })) => Ok(SiPunch::punches_from_payload(&payload[..], &host_info, now)),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{}: Encrypted message or wrong portnum", host_info.name),
            )),
        }?;

        let mut result = Vec::with_capacity(punches.len());
        let status = self
            .meshtastic_statuses
            .entry(mac_addr)
            .or_insert(MeshtasticRocStatus::new(host_info.name.clone()));
        for punch in punches.into_iter() {
            match punch {
                Ok(punch) => {
                    let mut punch = punch.clone();
                    status.punch(&punch);
                    if let Some(mac_addr) = self.meshtastic_override_mac.as_ref() {
                        punch.host_info.mac_address = mac_addr.clone();
                    }
                    result.push(punch);
                }
                Err(err) => {
                    error!("{}", err);
                }
            }
        }

        Ok(result)
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
        self.punches_impl(payload, mac_addr)
            .map_err(|err| err.into())
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
    pub fn punches_impl(
        &mut self,
        payload: &[u8],
        mac_addr: &str,
    ) -> std::io::Result<Vec<SiPunch>> {
        let punches = Punches::decode(payload)?;
        let host_info: HostInfo = HostInfo {
            name: self.resolve(mac_addr).to_owned(),
            mac_address: mac_addr.to_owned(),
        };
        let status = self.get_cellular_status(mac_addr);
        let now = Local::now().fixed_offset();
        let mut result = Vec::with_capacity(punches.punches.len());
        for punch in punches.punches {
            match Self::construct_punch(&punch.raw, &host_info, now) {
                Ok(si_punch) => {
                    status.punch(&si_punch);
                    result.push(si_punch);
                }
                Err(err) => {
                    error!("{}", err);
                }
            };
        }

        Ok(result)
    }

    fn construct_punch(
        payload: &[u8],
        host_info: &HostInfo,
        now: DateTime<FixedOffset>,
    ) -> std::io::Result<SiPunch> {
        let length = payload.len();
        Ok(SiPunch::from_raw(
            payload.try_into().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Wrong length of chunk={length}"),
                )
            })?,
            &host_info,
            now,
        ))
    }

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

#[cfg(test)]
mod test_punch {
    use std::collections::HashMap;

    use prost::Message;

    use crate::protobufs::{Punch, Punches};

    use super::MessageHandler;

    #[test]
    fn test_punches() {
        let punches = Punches {
            punches: vec![Punch {
                raw: b"\x12\x43".to_vec(),
            }],
            sending_timestamp: None,
        };
        let message = punches.encode_to_vec();

        let mut handler = MessageHandler::new(HashMap::new(), None);
        let punches = handler.punches_impl(&message[..], "").unwrap();
        assert_eq!(punches.len(), 0);
    }
}
