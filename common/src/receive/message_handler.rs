extern crate std;

use chrono::DateTime;
use chrono::prelude::*;
use femtopb::Message as _;
use log::error;
use log::info;
use meshtastic::Message as MeshtasticMessage;
use meshtastic::protobufs::mesh_packet::PayloadVariant;
use meshtastic::protobufs::{Data, MeshPacket, PortNum, ServiceEnvelope};
use std::borrow::ToOwned;
use std::collections::HashMap;
use std::format;
use std::string::String;
use std::vec::Vec;

use crate::error::Error;
use crate::logs::CellularLogMessage;
use crate::meshtastic::{MshLogMessage, MshMetrics, PositionName};
use crate::proto::{Punches, Status};
use crate::punch::{SiPunch, SiPunchLog};
use crate::receive::state::{CellularRocStatus, MeshtasticRocStatus, NodeInfo};
use crate::system_info::{HostInfo, MacAddress};

pub struct MessageHandler {
    dns: HashMap<MacAddress, String>,
    cellular_statuses: HashMap<MacAddress, CellularRocStatus>,
    meshtastic_statuses: HashMap<MacAddress, MeshtasticRocStatus>,
}

impl MessageHandler {
    pub fn new(dns: Vec<(String, String)>) -> Result<Self, Error> {
        let dns = dns
            .into_iter()
            .map(|(mac, name)| match mac.len() {
                8 => Ok((
                    // TODO: remove unwrap
                    MacAddress::Meshtastic(
                        u32::from_str_radix(&mac, 16).map_err(|_| Error::ParseError)?,
                    ),
                    name,
                )),
                12 => Ok((
                    MacAddress::Full(u64::from_str_radix(&mac, 16).map_err(|_| Error::ParseError)?),
                    name,
                )),
                _ => Err(Error::ValueError),
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            dns,
            meshtastic_statuses: HashMap::new(),
            cellular_statuses: HashMap::new(),
        })
    }

    pub fn status_update(
        &mut self,
        payload: &[u8],
        mac_addr: MacAddress,
    ) -> Result<CellularLogMessage, Error> {
        let status_proto = Status::decode(payload).map_err(|_| Error::ParseError)?;
        let log_message =
            CellularLogMessage::from_proto(status_proto, mac_addr, self.resolve(mac_addr), &Local)?;

        let status = self.get_cellular_status(mac_addr);
        match &log_message {
            CellularLogMessage::MCH(mch_log) => {
                let mch = &mch_log.mini_call_home;
                if let Some(signal_info) = mch.signal_info {
                    status.mqtt_connect_update(signal_info);
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
        Ok(log_message)
    }

    pub fn punches(
        &mut self,
        payload: &[u8],
        mac_address: MacAddress,
    ) -> Result<Vec<SiPunchLog>, Error> {
        let punches = Punches::decode(payload).map_err(|_| Error::ParseError)?;
        let host_info = HostInfo::new(self.resolve(mac_address), mac_address)?;
        let status = self.get_cellular_status(mac_address);
        let now = Local::now().fixed_offset();
        let mut result = Vec::with_capacity(punches.punches.len());
        for punch in punches.punches.into_iter().flatten() {
            match punch.raw.try_into() {
                Ok(bytes) => {
                    let si_punch = SiPunchLog::from_raw(bytes, host_info.clone(), now);
                    status.punch(&si_punch.punch);
                    result.push(si_punch);
                }
                Err(_) => {
                    error!("Wrong length of chunk={}", punch.raw.len());
                }
            }
        }

        Ok(result)
    }

    fn msh_roc_status(&mut self, host_info: &HostInfo) -> &mut MeshtasticRocStatus {
        self.meshtastic_statuses
            .entry(host_info.mac_address)
            .or_insert(MeshtasticRocStatus::new(host_info.name.as_str().to_owned()))
    }

    fn resolve(&self, mac_addr: MacAddress) -> &str {
        self.dns.get(&mac_addr).map(|x| x.as_str()).unwrap_or("Unknown")
    }

    pub fn msh_status_mesh_packet(
        &mut self,
        payload: &[u8],
        now: DateTime<FixedOffset>,
        recv_mac_address: Option<u32>,
    ) {
        let recv_position = recv_mac_address
            .and_then(|mac_addr| self.get_position_name(MacAddress::Meshtastic(mac_addr)));
        let msh_log_message =
            MshLogMessage::from_mesh_packet(payload, now, &self.dns, recv_position);
        self.msh_status_update(msh_log_message)
    }

    pub fn msh_status_service_envelope(
        &mut self,
        payload: &[u8],
        now: DateTime<FixedOffset>,
        recv_mac_address: Option<u32>,
    ) {
        let recv_position = recv_mac_address
            .and_then(|mac_addr| self.get_position_name(MacAddress::Meshtastic(mac_addr)));
        let msh_log_message =
            MshLogMessage::from_service_envelope(payload, now, &self.dns, recv_position);
        self.msh_status_update(msh_log_message)
    }

    fn msh_status_update(&mut self, log_message: Result<Option<MshLogMessage>, std::io::Error>) {
        match log_message {
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
            Err(err) => {
                error!("Failed to parse msh status proto: {}", err);
            }
            _ => {} // Ignored proto, maybe add a debug print?
        }
    }

    pub fn msh_serial_service_envelope(
        &mut self,
        payload: &[u8],
    ) -> std::io::Result<Vec<SiPunchLog>> {
        let service_envelope = ServiceEnvelope::decode(payload).map_err(std::io::Error::from)?;
        let packet = service_envelope.packet.ok_or(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Missing packet in ServiceEnvelope",
        ))?;
        self.msh_serial(packet)
    }

    pub fn msh_serial_mesh_packet(&mut self, payload: &[u8]) -> std::io::Result<Vec<SiPunchLog>> {
        let packet = MeshPacket::decode(payload).map_err(std::io::Error::from)?;
        self.msh_serial(packet)
    }

    fn msh_serial(&mut self, packet: MeshPacket) -> std::io::Result<Vec<SiPunchLog>> {
        let mac_address = MacAddress::Meshtastic(packet.from);
        const SERIAL_APP: i32 = PortNum::SerialApp as i32;
        let now = Local::now().fixed_offset();
        let host_info = HostInfo::new(self.resolve(mac_address), mac_address)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Too long name"))?;
        let Some(PayloadVariant::Decoded(Data {
            portnum: SERIAL_APP,
            payload,
            ..
        })) = packet.payload_variant
        else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{}: Encrypted message or wrong portnum", host_info.name),
            ));
        };

        let status = self.msh_roc_status(&host_info);
        let mut result = Vec::with_capacity(payload.len() / 20);

        let punches = SiPunch::punches_from_payload(&payload, now.date_naive(), now.offset());
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

    use crate::meshtastic::RssiSnr;
    use crate::proto::Punch;
    use crate::punch::SiPunch;
    use crate::receive::state::SignalInfo;

    use chrono::Local;
    use femtopb::Repeated;
    use meshtastic::protobufs::telemetry::Variant;
    use meshtastic::protobufs::{DeviceMetrics, MeshPacket, ServiceEnvelope, Telemetry};

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

        let mut handler = MessageHandler::new(Vec::new()).unwrap();
        // TODO: should propagate errors
        let punches = handler.punches(&buf[..len], MacAddress::default()).unwrap();
        assert_eq!(punches.len(), 0);
    }

    #[test]
    fn test_punch() {
        let time = DateTime::parse_from_rfc3339("2023-11-23T10:00:03.793+01:00").unwrap();
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

        let mut handler = MessageHandler::new(Vec::new()).unwrap();
        let punch_logs = handler.punches(&buf[..len], MacAddress::default()).unwrap();
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
        let time = DateTime::parse_from_rfc3339("2023-11-23T10:00:03+01:00").unwrap();
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
        let mut handler = MessageHandler::new(Vec::new()).unwrap();
        let punch_logs = handler.msh_serial_service_envelope(&message).unwrap();
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
            variant: Some(Variant::DeviceMetrics(DeviceMetrics {
                battery_level: Some(47),
                voltage: Some(3.712),
                ..Default::default()
            })),
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
        let mut handler = MessageHandler::new(Vec::new()).unwrap();
        handler.msh_status_service_envelope(&message, Local::now().fixed_offset(), None);
        let node_infos = handler.node_infos();
        assert_eq!(node_infos.len(), 1);
        assert_eq!(
            node_infos[0].signal_info,
            SignalInfo::Meshtastic(RssiSnr {
                rssi_dbm: -98,
                snr: 4.0,
                distance: None
            })
        );

        let envelope = ServiceEnvelope {
            packet: Some(MeshPacket {
                payload_variant: Some(PayloadVariant::Decoded(data)),
                ..Default::default()
            }),
            ..Default::default()
        };
        handler.msh_status_service_envelope(
            &envelope.encode_to_vec(),
            Local::now().fixed_offset(),
            None,
        );
        let node_infos = handler.node_infos();
        assert_eq!(node_infos.len(), 1);
        assert_eq!(node_infos[0].signal_info, SignalInfo::Unknown);
    }

    #[test]
    fn test_meshtastic_serial_and_status() {
        let time = DateTime::parse_from_rfc3339("2023-11-23T10:00:03+01:00").unwrap();
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

        let mut handler = MessageHandler::new(Vec::new()).unwrap();
        handler.msh_serial_service_envelope(&message).unwrap();

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
        handler.msh_status_service_envelope(&message, Local::now().fixed_offset(), None);
        let node_infos = handler.node_infos();
        assert_eq!(node_infos.len(), 1);
    }
}
