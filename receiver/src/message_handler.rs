use chrono::DateTime;
use chrono::prelude::*;
use femtopb::Message as _;
use log::error;
#[cfg(feature = "meshtastic")]
use meshtastic::Message as MeshtasticMessage;
#[cfg(feature = "meshtastic")]
use meshtastic::protobufs::mesh_packet::PayloadVariant;
#[cfg(feature = "meshtastic")]
use meshtastic::protobufs::{Data, MeshPacket, PortNum, ServiceEnvelope};
use std::collections::HashMap;
use tokio::sync::mpsc::{Receiver, Sender, channel};

use yaroc_common::error::Error;
use yaroc_common::proto::{Punches, Status};
use yaroc_common::punch::SiPunchLog;
use yaroc_common::system_info::{HostInfo, MacAddress};

use crate::logs::CellularLogMessage;
#[cfg(feature = "meshtastic")]
use crate::meshtastic::{MeshtasticLog, MshMetrics, PositionName};
use crate::mqtt::{Message as MqttMessage, MqttConfig, MqttReceiver};
use crate::state::CellularRocStatus;

pub struct MessageHandler {
    dns: HashMap<MacAddress, String>,
    mqtt_receiver: Option<MqttReceiver>,
    cellular_statuses: HashMap<MacAddress, CellularRocStatus>,
    #[cfg(feature = "meshtastic")]
    meshtastic_statuses: HashMap<MacAddress, crate::state::MeshtasticRocStatus>,
    _msh_dev_event_rx: Receiver<String>,
    msh_dev_event_tx: Sender<String>,
}

#[derive(Debug)]
pub enum Message {
    CellularLog(CellularLogMessage),
    SiPunches(Vec<SiPunchLog>),
    #[cfg(feature = "meshtastic")]
    MeshtasticLog,
}

pub struct MshDevNotifier {
    dev_event_tx: Sender<String>,
}

impl MshDevNotifier {
    pub async fn add_device(&self, port: String) -> yaroc_common::Result<()> {
        self.dev_event_tx.send(port).await.map_err(|_| Error::ChannelSendError)
    }
}

impl MessageHandler {
    pub fn new(dns: Vec<(String, MacAddress)>, mqtt_config: Option<MqttConfig>) -> Self {
        let macs: Vec<&MacAddress> = dns.iter().map(|(_, mac)| mac).collect();
        let mqtt_receiver = mqtt_config.map(|config| MqttReceiver::new(config, macs.into_iter()));
        let (tx, rx) = channel(10);
        Self {
            dns: dns.into_iter().map(|(name, mac)| (mac, name)).collect(),
            mqtt_receiver,
            #[cfg(feature = "meshtastic")]
            meshtastic_statuses: HashMap::new(),
            cellular_statuses: HashMap::new(),
            msh_dev_event_tx: tx,
            _msh_dev_event_rx: rx,
        }
    }

    pub async fn next_message(&mut self) -> Result<Message, Error> {
        let receiver = self.mqtt_receiver.as_mut().ok_or(Error::ValueError)?;
        let mqtt_message = receiver.next_message().await?;
        self.process_message(mqtt_message)
    }

    fn process_message(&mut self, mqtt_message: MqttMessage) -> yaroc_common::Result<Message> {
        match mqtt_message {
            MqttMessage::CellularStatus(mac_address, _, payload) => {
                self.status_update(&payload, mac_address).map(Message::CellularLog)
            }
            MqttMessage::Punches(mac_address, now, payload) => {
                self.punches(mac_address, now, &payload).map(Message::SiPunches)
            }
            #[cfg(feature = "meshtastic")]
            MqttMessage::MeshtasticSerial(_, payload) => {
                self.msh_serial_service_envelope(&payload).map(Message::SiPunches)
            }
            #[cfg(feature = "meshtastic")]
            MqttMessage::MeshtasticStatus(recv_mac_address, now, payload) => {
                self.msh_status_service_envelope(&payload, now, recv_mac_address);
                Ok(Message::MeshtasticLog)
            }
        }
    }

    pub fn meshtastic_device_notifier(&self) -> MshDevNotifier {
        MshDevNotifier {
            dev_event_tx: self.msh_dev_event_tx.clone(),
        }
    }

    fn status_update(
        &mut self,
        payload: &[u8],
        mac_address: MacAddress,
    ) -> Result<CellularLogMessage, Error> {
        let status_proto = Status::decode(payload).map_err(|_| Error::ProtobufParseError)?;
        let log_message =
            CellularLogMessage::from_proto(status_proto, self.resolve(mac_address), &Local)?;

        let status = self.get_cellular_status(mac_address);
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
            CellularLogMessage::Disconnected { .. } => {
                status.disconnect();
            }
            _ => {}
        }
        Ok(log_message)
    }

    fn punches(
        &mut self,
        mac_address: MacAddress,
        now: DateTime<Local>,
        payload: &[u8],
    ) -> Result<Vec<SiPunchLog>, Error> {
        let now = now.into();
        let punches = Punches::decode(payload).map_err(|_| Error::ProtobufParseError)?;
        let host_info = self.resolve(mac_address);
        let status = self.get_cellular_status(mac_address);
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

    #[cfg(feature = "meshtastic")]
    fn msh_roc_status(&mut self, host_info: &HostInfo) -> &mut crate::state::MeshtasticRocStatus {
        self.meshtastic_statuses.entry(host_info.mac_address).or_insert(
            crate::state::MeshtasticRocStatus::new(host_info.name.as_str().to_owned()),
        )
    }

    fn resolve(&self, mac_address: MacAddress) -> HostInfo {
        let name = self.dns.get(&mac_address).map(|x| x.as_str()).unwrap_or("Unknown");
        HostInfo::new(name, mac_address)
    }

    #[allow(dead_code)]
    #[cfg(feature = "meshtastic")]
    fn msh_status_mesh_packet(
        &mut self,
        payload: &[u8],
        now: DateTime<FixedOffset>,
        recv_mac_address: Option<u32>,
    ) {
        let recv_position = recv_mac_address
            .and_then(|mac_addr| self.get_position_name(MacAddress::Meshtastic(mac_addr)));
        let meshtastic_log =
            MeshtasticLog::from_mesh_packet(payload, now, &self.dns, recv_position);
        self.msh_status_update(meshtastic_log)
    }

    #[cfg(feature = "meshtastic")]
    fn msh_status_service_envelope(
        &mut self,
        payload: &[u8],
        now: DateTime<Local>,
        recv_mac_address: MacAddress,
    ) {
        let recv_position = self.get_position_name(recv_mac_address);
        let meshtastic_log =
            MeshtasticLog::from_service_envelope(payload, now.into(), &self.dns, recv_position);
        self.msh_status_update(meshtastic_log)
    }

    #[cfg(feature = "meshtastic")]
    fn msh_status_update(&mut self, log_message: yaroc_common::Result<Option<MeshtasticLog>>) {
        match log_message {
            Ok(Some(log_message)) => {
                log::info!("{}", log_message);
                let status = self.msh_roc_status(&log_message.host_info);
                match log_message.metrics {
                    MshMetrics::Battery { percent, .. } => {
                        status.update_battery(percent);
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

    #[cfg(feature = "meshtastic")]
    /// Process Meshtastic message of the serial module wrapped in ServiceEnvelope.
    fn msh_serial_service_envelope(
        &mut self,
        payload: &[u8],
    ) -> yaroc_common::Result<Vec<SiPunchLog>> {
        let service_envelope =
            ServiceEnvelope::decode(payload).map_err(|_| Error::ProtobufParseError)?;
        let packet = service_envelope.packet.ok_or(Error::ProtobufParseError)?;
        self.msh_serial(packet)
    }

    #[cfg(feature = "meshtastic")]
    /// Process Meshtastic message of the serial module given as MeshPacket.
    #[allow(dead_code)]
    fn msh_serial_mesh_packet(&mut self, payload: &[u8]) -> yaroc_common::Result<Vec<SiPunchLog>> {
        let packet = MeshPacket::decode(payload).map_err(|_| Error::ProtobufParseError)?;
        self.msh_serial(packet)
    }

    #[cfg(feature = "meshtastic")]
    fn msh_serial(&mut self, packet: MeshPacket) -> yaroc_common::Result<Vec<SiPunchLog>> {
        let mac_address = MacAddress::Meshtastic(packet.from);
        const SERIAL_APP: i32 = PortNum::SerialApp as i32;
        let now = Local::now().fixed_offset();
        let host_info = self.resolve(mac_address);
        let Some(PayloadVariant::Decoded(Data {
            portnum: SERIAL_APP,
            payload,
            ..
        })) = packet.payload_variant
        else {
            // Encrypted message or wrong portnum
            return Err(Error::ParseError);
        };

        let status = self.msh_roc_status(&host_info);
        let mut result = Vec::with_capacity(payload.len() / 20);

        let punches = yaroc_common::punch::SiPunch::punches_from_payload(
            &payload,
            now.date_naive(),
            now.offset(),
        );
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

    #[cfg(feature = "meshtastic")]
    pub fn node_infos(&self) -> Vec<crate::state::NodeInfo> {
        let mut res: Vec<_> = self
            .meshtastic_statuses
            .values()
            .map(|status| status.serialize())
            .chain(self.cellular_statuses.values().map(|status| status.serialize()))
            .collect();
        res.sort_by(|a, b| a.name.cmp(&b.name));
        res
    }

    #[cfg(feature = "meshtastic")]
    fn get_position_name(&self, mac_address: MacAddress) -> Option<PositionName> {
        let status = self.meshtastic_statuses.get(&mac_address)?;
        status
            .position
            .as_ref()
            .map(|position| PositionName::new(position, &status.name))
    }

    fn get_cellular_status(&mut self, mac_addr: MacAddress) -> &mut CellularRocStatus {
        let host_info = self.resolve(mac_addr);
        self.cellular_statuses
            .entry(mac_addr)
            .or_insert(CellularRocStatus::new(host_info))
    }
}

#[cfg(test)]
mod test_punch {
    use super::*;

    use yaroc_common::proto::Punch;
    use yaroc_common::punch::SiPunch;

    use chrono::Local;
    use femtopb::Repeated;

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

        let mut handler = MessageHandler::new(Vec::new(), None);
        let now = Local::now();
        // TODO: should propagate errors
        let punches = handler.punches(MacAddress::default(), now, &buf[..len]).unwrap();
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

        let mut handler = MessageHandler::new(Vec::new(), None);
        let now = Local::now();
        let punch_logs = handler.punches(MacAddress::default(), now, &buf[..len]).unwrap();
        assert_eq!(punch_logs.len(), 1);
        assert_eq!(punch_logs[0].punch.code, 47);
        assert_eq!(punch_logs[0].punch.card, 1715004);
    }
}

#[cfg(feature = "meshtastic")]
#[cfg(test)]
mod test_meshtastic {
    use crate::meshtastic::RssiSnr;
    use crate::state::SignalInfo;
    use meshtastic::protobufs::telemetry::Variant;
    use meshtastic::protobufs::{DeviceMetrics, MeshPacket, ServiceEnvelope, Telemetry};

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
        let mut handler = MessageHandler::new(Vec::new(), None);
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
        let mut handler = MessageHandler::new(Vec::new(), None);
        handler.msh_status_service_envelope(&message, Local::now(), MacAddress::default());
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
            Local::now(),
            MacAddress::default(),
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

        let mut handler = MessageHandler::new(Vec::new(), None);
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
        handler.msh_status_service_envelope(&message, Local::now(), MacAddress::default());
        let node_infos = handler.node_infos();
        assert_eq!(node_infos.len(), 1);
    }
}
