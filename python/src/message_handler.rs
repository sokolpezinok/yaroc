use femtopb::Message as _;
use log::error;
use log::info;
use meshtastic::protobufs::mesh_packet::PayloadVariant;
use meshtastic::protobufs::{Data, PortNum, ServiceEnvelope};
use meshtastic::Message as MeshtasticMessage;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use std::collections::HashMap;
use yaroc_common::error::Error;
use yaroc_common::proto::Punches;
use yaroc_common::proto::Status;

use chrono::prelude::*;
use chrono::DateTime;

use crate::logs::MacAddress;
use crate::logs::{CellularLogMessage, HostInfo, PositionName};
use crate::meshtastic::MshLogMessage;
use crate::meshtastic::MshMetrics;
use crate::punch::SiPunch;
use crate::punch::SiPunchLog;
use crate::status::{CellularRocStatus, MeshtasticRocStatus, NodeInfo};

#[pyclass]
pub struct MessageHandler {
    dns: HashMap<MacAddress, String>,
    cellular_statuses: HashMap<MacAddress, CellularRocStatus>,
    meshtastic_statuses: HashMap<MacAddress, MeshtasticRocStatus>,
}

#[pymethods]
impl MessageHandler {
    #[staticmethod]
    pub fn new(dns: Vec<(String, String)>) -> Self {
        let dns = dns
            .into_iter()
            .map(|(mac, name)| match mac.len() {
                8 => (
                    // TODO: remove unwrap
                    MacAddress::Meshtastic(u32::from_str_radix(&mac, 16).unwrap()),
                    name,
                ),
                12 => (
                    MacAddress::Full(u64::from_str_radix(&mac, 16).unwrap()),
                    name,
                ),
                _ => (MacAddress::default(), name), // TODO: error
            })
            .collect();
        Self {
            dns,
            meshtastic_statuses: HashMap::new(),
            cellular_statuses: HashMap::new(),
        }
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
        recv_mac_address: Option<u32>,
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
    pub fn punches_py(&mut self, payload: &[u8], mac_addr: u64) -> PyResult<Vec<SiPunchLog>> {
        self.punches(payload, mac_addr)
            .map_err(|err| PyValueError::new_err(format!("{err}")))
    }

    pub fn status_update(&mut self, payload: &[u8], mac_addr: u64) -> PyResult<()> {
        let status_proto = Status::decode(payload)
            .map_err(|_| PyValueError::new_err("Status proto decoding error"))?;
        let mac_addr = MacAddress::Full(mac_addr);
        let log_message =
            CellularLogMessage::from_proto(status_proto, mac_addr, self.resolve(mac_addr), &Local)
                .map_err(
                    |err| PyValueError::new_err(format!("{}", err)), // TODO: which?
                )?;
        info!("{}", log_message);

        let status = self.get_cellular_status(mac_addr);
        match log_message {
            CellularLogMessage::MCH(mch_log) => {
                let mch = mch_log.mini_call_home;
                if let Some(rssi_dbm) = mch.rssi_dbm {
                    status.mqtt_connect_update(
                        rssi_dbm,
                        mch.cellid.unwrap_or_default(),
                        mch.snr_cb,
                    );
                }
                if let Some(batt_mv) = mch.batt_mv {
                    status.update_voltage(f64::from(batt_mv) / 1000.);
                }
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
    pub fn punches(&mut self, payload: &[u8], mac_address: u64) -> Result<Vec<SiPunchLog>, Error> {
        let punches = Punches::decode(payload).map_err(|_| Error::ParseError)?;
        let mac_address_full = MacAddress::Full(mac_address);
        let host_info: HostInfo =
            HostInfo::new(self.resolve(mac_address_full).to_owned(), mac_address);
        let status = self.get_cellular_status(mac_address_full);
        let now = Local::now().fixed_offset();
        let mut result = Vec::with_capacity(punches.punches.len());
        for punch in punches.punches.into_iter().flatten() {
            match Self::construct_punch(punch.raw, &host_info, now) {
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

    fn msh_roc_status(&mut self, host_info: &HostInfo) -> &mut MeshtasticRocStatus {
        self.meshtastic_statuses
            .entry(host_info.mac_address)
            .or_insert(MeshtasticRocStatus::new(host_info.name.clone()))
    }

    fn resolve(&self, mac_addr: MacAddress) -> &str {
        self.dns.get(&mac_addr).map(|x| x.as_str()).unwrap_or("Unknown")
    }

    pub fn msh_status_update(
        &mut self,
        payload: &[u8],
        now: DateTime<FixedOffset>,
        recv_mac_address: Option<u32>,
    ) {
        let recv_position = recv_mac_address
            .and_then(|mac_addr| self.get_position_name(MacAddress::Meshtastic(mac_addr)));
        let msh_log_message =
            MshLogMessage::from_mesh_packet(payload, now, &self.dns, recv_position);
        match msh_log_message {
            Err(err) => {
                error!("Failed to parse msh status proto: {}", err);
            }
            Ok(Some(log_message)) => {
                info!("{}", log_message);
                let status = self.msh_roc_status(&log_message.host_info);
                match log_message.metrics {
                    MshMetrics::VoltageBattery(_, battery) => {
                        status.update_battery(battery);
                    }
                    MshMetrics::Position(position) => status.position = Some(position),
                    // TODO: handle temperature
                    _ => {}
                }
                if let Some(rssi_snr) = log_message.rssi_snr.as_ref() {
                    status.update_rssi_snr(rssi_snr.clone());
                } else {
                    status.clear_rssi_snr();
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
        let mac_address = MacAddress::Meshtastic(packet.from);
        const SERIAL_APP: i32 = PortNum::SerialApp as i32;
        let now = Local::now().fixed_offset();
        let host_info = HostInfo {
            name: self.resolve(mac_address).to_owned(),
            mac_address,
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

        let status = self.msh_roc_status(&host_info);
        let mut result = Vec::with_capacity(punches.len());
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

    fn get_position_name(&self, mac_address: MacAddress) -> Option<PositionName> {
        let status = self.meshtastic_statuses.get(&mac_address)?;
        status
            .position
            .as_ref()
            .map(|position| PositionName::new(position, &status.name))
    }

    fn get_cellular_status(&mut self, mac_addr: MacAddress) -> &mut CellularRocStatus {
        let name = self.resolve(mac_addr).to_owned();
        self.cellular_statuses.entry(mac_addr).or_insert(CellularRocStatus::new(name))
    }
}

#[cfg(test)]
mod test_punch {
    use super::*;
    use chrono::{Local, NaiveDateTime};
    use femtopb::Repeated;
    use meshtastic::protobufs::telemetry::Variant;
    use meshtastic::protobufs::{MeshPacket, ServiceEnvelope, Telemetry};
    use yaroc_common::proto::Punch;
    use yaroc_common::punch::SiPunch;

    #[test]
    fn test_wrong_punch() {
        let punches_slice = &[Punch {
            raw: b"\x12\x43",
            ..Default::default()
        }];
        let punches = Punches {
            punches: Repeated::from_slice(punches_slice),
            ..Default::default()
        };
        let mut buf = [0u8; 30];
        let len = punches.encoded_len();
        punches.encode(&mut buf.as_mut_slice()).unwrap();

        let mut handler = MessageHandler::new(Vec::new());
        // TODO: should propagate errors
        let punches = handler.punches(&buf[..len], 0x1234).unwrap();
        assert_eq!(punches.len(), 0);
    }

    #[test]
    fn test_punch() {
        let time = NaiveDateTime::parse_from_str("2023-11-23 10:00:03.793", "%Y-%m-%d %H:%M:%S%.f")
            .unwrap();
        let punch = SiPunch::new(1715004, 47, time, 2).raw;
        let punches_slice = &[Punch {
            raw: &punch,
            ..Default::default()
        }];
        let punches = Punches {
            punches: Repeated::from_slice(punches_slice),
            ..Default::default()
        };
        let mut buf = [0u8; 30];
        let len = punches.encoded_len();
        punches.encode(&mut buf.as_mut_slice()).unwrap();

        let mut handler = MessageHandler::new(Vec::new());
        let punch_logs = handler.punches(&buf[..len], 0x1234).unwrap();
        assert_eq!(punch_logs.len(), 1);
        assert_eq!(punch_logs[0].punch.code, 47);
        assert_eq!(punch_logs[0].punch.card, 1715004);
    }

    fn envelope(from: u32, data: Data) -> ServiceEnvelope {
        ServiceEnvelope {
            packet: Some(MeshPacket {
                payload_variant: Some(PayloadVariant::Decoded(data)),
                from,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn test_meshtastic_serial() {
        let time =
            NaiveDateTime::parse_from_str("2023-11-23 10:00:03", "%Y-%m-%d %H:%M:%S").unwrap();
        let punch = SiPunch::new(1715004, 47, time, 2).raw;

        let message = envelope(
            0xdeadbeef,
            Data {
                portnum: PortNum::SerialApp as i32,
                payload: punch.to_vec(),
                ..Default::default()
            },
        )
        .encode_to_vec();
        let mut handler = MessageHandler::new(Vec::new());
        let punch_logs = handler.msh_serial_msg(&message).unwrap();
        assert_eq!(punch_logs.len(), 1);
        assert_eq!(punch_logs[0].punch.code, 47);
        assert_eq!(punch_logs[0].punch.card, 1715004);
        let node_infos = handler.node_infos();
        assert_eq!(node_infos.len(), 1);
        assert_eq!(
            node_infos[0].last_punch.unwrap().time(),
            NaiveTime::from_hms_opt(10, 0, 3).unwrap()
        );
    }

    #[test]
    fn test_meshtastic_status() {
        let telemetry = Telemetry {
            time: 1735157442,
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
        let message = envelope1.encode_to_vec();
        let mut handler = MessageHandler::new(Vec::new());
        handler.msh_status_update(&message, Local::now().fixed_offset(), None);
        let node_infos = handler.node_infos();
        assert_eq!(node_infos.len(), 1);
        assert_eq!(node_infos[0].rssi_dbm, Some(-98));
        assert_eq!(node_infos[0].snr_db, Some(4.0));

        let envelope = ServiceEnvelope {
            packet: Some(MeshPacket {
                payload_variant: Some(PayloadVariant::Decoded(data)),
                ..Default::default()
            }),
            ..Default::default()
        };
        handler.msh_status_update(&envelope.encode_to_vec(), Local::now().fixed_offset(), None);
        let node_infos = handler.node_infos();
        assert_eq!(node_infos.len(), 1);
        assert_eq!(node_infos[0].rssi_dbm, None);
    }

    #[test]
    fn test_meshtastic_serial_and_status() {
        let time =
            NaiveDateTime::parse_from_str("2023-11-23 10:00:03", "%Y-%m-%d %H:%M:%S").unwrap();
        let punch = SiPunch::new(1715004, 47, time, 2).raw;

        let message = envelope(
            0xdeadbeef,
            Data {
                portnum: PortNum::SerialApp as i32,
                payload: punch.to_vec(),
                ..Default::default()
            },
        )
        .encode_to_vec();

        let mut handler = MessageHandler::new(Vec::new());
        handler.msh_serial_msg(&message).unwrap();

        let telemetry = Telemetry {
            time: 1735157442,
            variant: Some(Variant::DeviceMetrics(Default::default())),
        };
        let data = Data {
            portnum: PortNum::TelemetryApp as i32,
            payload: telemetry.encode_to_vec(),
            ..Default::default()
        };
        let message = envelope(0xdeadbeef, data).encode_to_vec();
        handler.msh_status_update(&message, Local::now().fixed_offset(), None);
        let node_infos = handler.node_infos();
        assert_eq!(node_infos.len(), 1);
    }
}
