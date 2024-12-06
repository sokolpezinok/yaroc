use log::error;
use log::info;
use meshtastic::protobufs::mesh_packet::PayloadVariant;
use meshtastic::protobufs::{Data, PortNum, ServiceEnvelope};
use meshtastic::Message as MeshtasticMessage;
use prost::Message;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use std::collections::HashMap;

use chrono::prelude::*;
use chrono::DateTime;

use crate::logs::{CellularLogMessage, HostInfo, PositionName};
use crate::meshtastic::MshLogMessage;
use crate::protobufs::{Punches, Status};
use crate::punch::SiPunch;
use crate::punch::SiPunchLog;
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
    #[pyo3(signature = (dns, meshtastic_override_mac=None))]
    pub fn new(dns: HashMap<String, String>, meshtastic_override_mac: Option<String>) -> Self {
        Self {
            dns,
            meshtastic_statuses: HashMap::new(),
            cellular_statuses: HashMap::new(),
            meshtastic_override_mac,
        }
    }

    fn resolve(&self, mac_addr: &str) -> &str {
        self.dns.get(mac_addr).map(|x| x.as_str()).unwrap_or("Unknown")
    }

    #[pyo3(name = "meshtastic_serial_msg")]
    pub fn meshtastic_serial_msg_py(&mut self, payload: &[u8]) -> PyResult<Vec<SiPunchLog>> {
        Ok(self.msh_serial_msg(payload)?)
    }

    #[pyo3(name = "meshtastic_status_update", signature = (payload, now, recv_mac_address=None))]
    pub fn meshtastic_status_update(
        &mut self,
        payload: &[u8],
        now: DateTime<FixedOffset>,
        recv_mac_address: Option<String>,
    ) {
        self.msh_status_update(payload, now, recv_mac_address);
    }

    pub fn node_infos(&self) -> Vec<NodeInfo> {
        let mut res: Vec<_> = self
            .meshtastic_statuses
            .values()
            .map(|status| status.serialize())
            .chain(self.cellular_statuses.values().map(|status| status.serialize()))
            .collect();
        res.sort_by(|a, b| a.name.cmp(&b.name));
        res
    }

    #[pyo3(name = "punches")]
    pub fn punches_py(&mut self, payload: &[u8], mac_addr: &str) -> PyResult<Vec<SiPunchLog>> {
        self.punches(payload, mac_addr).map_err(|err| err.into())
    }

    pub fn status_update(&mut self, payload: &[u8], mac_addr: &str) -> PyResult<()> {
        let status_proto =
            Status::decode(payload).map_err(|e| PyValueError::new_err(format!("{e}")))?;
        let log_message =
            CellularLogMessage::from_proto(status_proto, mac_addr, self.resolve(mac_addr)).ok_or(
                PyValueError::new_err("Missing fields in the Status proto"), // TODO: which?
            )?;
        info!("{}", log_message);

        let status = self.get_cellular_status(mac_addr);
        match log_message {
            CellularLogMessage::MCH(mch) => {
                if let Some(rssi_dbm) = mch.rssi_dbm {
                    status.mqtt_connect_update(rssi_dbm, mch.cellid.unwrap_or_default(), mch.snr);
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
    pub fn punches(&mut self, payload: &[u8], mac_addr: &str) -> std::io::Result<Vec<SiPunchLog>> {
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
                    status.punch(&si_punch.punch);
                    result.push(si_punch);
                }
                Err(err) => {
                    error!("{}", err);
                }
            }
        }

        Ok(result)
    }

    fn msh_roc_status(&mut self, host_info: HostInfo) -> &mut MeshtasticRocStatus {
        self.meshtastic_statuses
            .entry(host_info.mac_address)
            .or_insert(MeshtasticRocStatus::new(host_info.name))
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
                let status = self.msh_roc_status(log_message.host_info);
                if let Some(position) = log_message.position.as_ref() {
                    status.position = Some(position.clone())
                }
                if let Some(rssi_snr) = log_message.rssi_snr.as_ref() {
                    status.update_rssi_snr(rssi_snr.clone());
                } else {
                    status.clear_rssi_snr();
                }
                if let Some((_, battery)) = log_message.voltage_battery.as_ref() {
                    status.update_battery(*battery);
                }
            }
            _ => {}
        }
    }

    fn msh_serial_msg(&mut self, payload: &[u8]) -> std::io::Result<Vec<SiPunchLog>> {
        let service_envelope = ServiceEnvelope::decode(payload).map_err(std::io::Error::from)?;
        let packet = service_envelope.packet.ok_or(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Missing packet in ServiceEnvelope",
        ))?;
        let mac_addr = format!("{:8x}", packet.from);
        const SERIAL_APP: i32 = PortNum::SerialApp as i32;
        let now = Local::now().fixed_offset();
        let mut host_info = HostInfo {
            name: self.resolve(&mac_addr).to_owned(),
            mac_address: mac_addr.clone(),
        };
        let punches = match packet.payload_variant {
            Some(PayloadVariant::Decoded(Data {
                portnum: SERIAL_APP,
                payload,
                ..
            })) => Ok(SiPunch::punches_from_payload(&payload)),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{}: Encrypted message or wrong portnum", host_info.name),
            )),
        }?;

        let mut result = Vec::with_capacity(punches.len());
        if let Some(mac_addr) = self.meshtastic_override_mac.as_ref() {
            host_info.mac_address = mac_addr.clone();
        }
        let status = self.msh_roc_status(host_info.clone());
        for punch in punches.into_iter() {
            match punch {
                Ok(punch) => {
                    status.punch(&punch);
                    result.push(SiPunchLog {
                        latency: now - punch.time,
                        punch,
                        host_info: host_info.clone(),
                    });
                }
                Err(err) => {
                    error!("{}", err);
                }
            }
        }

        Ok(result)
    }

    fn construct_punch(
        payload: &[u8],
        host_info: &HostInfo,
        now: DateTime<FixedOffset>,
    ) -> std::io::Result<SiPunchLog> {
        let length = payload.len();
        Ok(SiPunchLog::from_raw(
            payload.try_into().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Wrong length of chunk={length}"),
                )
            })?,
            host_info,
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

    use chrono::{Local, NaiveDateTime};
    use meshtastic::protobufs::mesh_packet::PayloadVariant;
    use meshtastic::protobufs::telemetry::Variant;
    use meshtastic::protobufs::{Data, MeshPacket, PortNum, ServiceEnvelope, Telemetry};
    use meshtastic::Message as MeshtasticMessage;
    use prost::Message;

    use crate::protobufs::{Punch, Punches};

    use super::MessageHandler;

    #[test]
    fn test_wrong_punch() {
        let punches = Punches {
            punches: vec![Punch {
                raw: b"\x12\x43".to_vec(),
            }],
            sending_timestamp: None,
        };
        let message = punches.encode_to_vec();

        let mut handler = MessageHandler::new(HashMap::new(), None);
        let punches = handler.punches(&message, "").unwrap();
        assert_eq!(punches.len(), 0);
    }

    #[test]
    fn test_punch() {
        let time = NaiveDateTime::parse_from_str("2023-11-23 10:00:03.793", "%Y-%m-%d %H:%M:%S%.f")
            .unwrap();
        let punch = yaroc_common::punch::SiPunch::punch_to_bytes(1715004, 47, time, 2);
        let punches = Punches {
            punches: vec![Punch {
                raw: punch.to_vec(),
            }],
            sending_timestamp: None,
        };
        let message = punches.encode_to_vec();

        let mut handler = MessageHandler::new(HashMap::new(), None);
        let punch_logs = handler.punches(&message, "").unwrap();
        assert_eq!(punch_logs.len(), 1);
        assert_eq!(punch_logs[0].punch.code, 47);
        assert_eq!(punch_logs[0].punch.card, 1715004);
    }

    #[test]
    fn test_meshtastic_serial() {
        let time = NaiveDateTime::parse_from_str("2023-11-23 10:00:03.793", "%Y-%m-%d %H:%M:%S%.f")
            .unwrap();
        let punch = yaroc_common::punch::SiPunch::punch_to_bytes(1715004, 47, time, 2);

        const SERIAL_APP: i32 = PortNum::SerialApp as i32;
        let envelope = ServiceEnvelope {
            packet: Some(MeshPacket {
                to: 0xabcd,
                from: 0x1234,
                payload_variant: Some(PayloadVariant::Decoded(Data {
                    portnum: SERIAL_APP,
                    payload: punch.to_vec(),
                    ..Default::default()
                })),
                ..Default::default()
            }),
            ..Default::default()
        };

        let message = envelope.encode_to_vec();
        let mut handler = MessageHandler::new(HashMap::new(), None);
        let punch_logs = handler.msh_serial_msg(&message).unwrap();
        assert_eq!(punch_logs.len(), 1);
        assert_eq!(punch_logs[0].punch.code, 47);
        assert_eq!(punch_logs[0].punch.card, 1715004);
    }

    #[test]
    fn test_meshtastic_status() {
        let telemetry = Telemetry {
            time: 3,
            variant: Some(Variant::DeviceMetrics(Default::default())),
        };
        let data = Data {
            portnum: PortNum::TelemetryApp as i32,
            payload: telemetry.encode_to_vec(),
            ..Default::default()
        };
        let envelope1 = ServiceEnvelope {
            packet: Some(MeshPacket {
                payload_variant: Some(PayloadVariant::Decoded(data.clone())),
                rx_rssi: -98,
                rx_snr: 4.0,
                ..Default::default()
            }),
            ..Default::default()
        };
        let message1 = envelope1.encode_to_vec();
        let mut handler = MessageHandler::new(HashMap::new(), None);
        handler.msh_status_update(&message1, Local::now().fixed_offset(), None);
        let node_infos = handler.node_infos();
        assert_eq!(node_infos.len(), 1);
        assert_eq!(node_infos[0].rssi_dbm, Some(-98));

        let envelope2 = ServiceEnvelope {
            packet: Some(MeshPacket {
                payload_variant: Some(PayloadVariant::Decoded(data)),
                ..Default::default()
            }),
            ..Default::default()
        };
        handler.msh_status_update(
            &envelope2.encode_to_vec(),
            Local::now().fixed_offset(),
            None,
        );
        let node_infos = handler.node_infos();
        assert_eq!(node_infos.len(), 1);
        assert_eq!(node_infos[0].rssi_dbm, None);
    }
}
