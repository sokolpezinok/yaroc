use chrono::DateTime;
use chrono::prelude::*;
use femtopb::Message as _;
use log::{debug, error, info, trace};
use meshtastic::Message as MeshtasticMessage;
use meshtastic::protobufs::mesh_packet::PayloadVariant;
use meshtastic::protobufs::{Data, MeshPacket, PortNum, ServiceEnvelope};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

use crate::error::Error;
use crate::logs::{CellularLogMessage, SiPunchLog};
use crate::meshtastic::{MeshtasticLog, MshMetrics, PositionName, RssiSnr};
use crate::meshtastic::{POSITION_APP, SERIAL_APP, TELEMETRY_APP};
use crate::mqtt::Message as MqttMessage;
use crate::system_info::{HostInfo, MacAddress};
use yaroc_common::proto::{Punches, Status};
use yaroc_common::punch::SiPunch;
use yaroc_common::status::CellSignalInfo;

#[derive(Clone, Debug, PartialEq)]
pub enum SignalInfo {
    Unknown,
    Cell(CellSignalInfo),
    Meshtastic(RssiSnr),
}

#[derive(Clone, Debug)]
pub struct NodeInfo {
    pub name: String,
    pub signal_info: SignalInfo,
    pub codes: Vec<u16>,
    pub last_update: Option<DateTime<FixedOffset>>,
    pub last_punch: Option<DateTime<FixedOffset>>,
}

#[derive(Default, Clone)]
pub struct CellularNodeStatus {
    host_info: HostInfo,
    state: Option<CellSignalInfo>,
    voltage: Option<f64>,
    codes: HashSet<u16>,
    last_update: Option<DateTime<FixedOffset>>,
    last_punch: Option<DateTime<FixedOffset>>,
}

impl CellularNodeStatus {
    pub fn new(host_info: HostInfo) -> Self {
        Self {
            host_info,
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
            None => SignalInfo::Unknown,
        };

        NodeInfo {
            name: self.host_info.name.to_string(),
            signal_info,
            codes: self.codes.iter().copied().collect(),
            last_update: self.last_update,
            last_punch: self.last_punch,
        }
    }
}

#[derive(Default, Clone)]
pub struct MeshtasticNodeStatus {
    pub name: String,
    battery: Option<u32>,
    pub rssi_snr: Option<RssiSnr>,
    pub position: Option<yaroc_common::status::Position>,
    codes: HashSet<u16>,
    last_update: Option<DateTime<FixedOffset>>,
    last_punch: Option<DateTime<FixedOffset>>,
}

impl MeshtasticNodeStatus {
    pub fn new(name: String) -> Self {
        Self {
            name,
            ..Default::default()
        }
    }

    pub fn update_battery(&mut self, percent: u32) {
        self.battery = Some(percent);
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
            None => SignalInfo::Unknown,
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

#[derive(Debug)]
pub enum Event {
    CellularLog(CellularLogMessage),
    SiPunches(Vec<SiPunchLog>),
    MeshtasticLog(MeshtasticLog),
    NodeInfos(Vec<NodeInfo>),
}

pub struct FleetState {
    dns: HashMap<MacAddress, String>,
    cellular_statuses: HashMap<MacAddress, CellularNodeStatus>,
    meshtastic_statuses: HashMap<MacAddress, MeshtasticNodeStatus>,
    node_infos_interval: Duration,
    last_node_info_push: Instant,
    new_node: Notify,
}

impl Default for FleetState {
    fn default() -> Self {
        Self {
            dns: HashMap::new(),
            cellular_statuses: HashMap::new(),
            meshtastic_statuses: HashMap::new(),
            node_infos_interval: Duration::from_secs(60),
            last_node_info_push: Instant::now(),
            new_node: Notify::new(),
        }
    }
}

impl FleetState {
    pub fn new(dns: Vec<(String, MacAddress)>, node_infos_interval: Duration) -> Self {
        Self {
            dns: dns.into_iter().map(|(name, mac)| (mac, name)).collect(),
            node_infos_interval,
            ..Default::default()
        }
    }

    /// Process a MQTT message.
    ///
    /// # Aguments
    ///
    /// * `mqtt_message` - The MQTT message.
    pub fn process_message(&mut self, mqtt_message: MqttMessage) -> crate::Result<Option<Event>> {
        match mqtt_message {
            MqttMessage::CellularStatus(mac_address, now, payload) => self
                .status_update(&payload, mac_address, now.into())
                .map(|msg| Some(Event::CellularLog(msg))),
            MqttMessage::Punches(mac_address, now, payload) => {
                self.punches(&payload, mac_address, now).map(|msg| Some(Event::SiPunches(msg)))
            }
            MqttMessage::MeshtasticSerial(now, payload) => self
                .msh_serial_service_envelope(&payload, now)
                .map(|msg| Some(Event::SiPunches(msg))),
            MqttMessage::MeshtasticStatus(recv_mac_address, now, payload) => self
                .msh_status_service_envelope(&payload, now, recv_mac_address)
                .map(|msg| msg.map(Event::MeshtasticLog)),
        }
    }

    /// Process a MeshPacket from a meshtastic mesh.
    ///
    /// # Aguments
    ///
    /// * `mesh_packet` - The MeshPacket parsed proto.
    /// * `recv_mac_address` - Optional receiver MAC address (if known).
    pub fn process_mesh_packet(
        &mut self,
        mesh_packet: MeshPacket,
        recv_mac_address: Option<MacAddress>,
    ) -> crate::Result<Option<Event>> {
        let now = Local::now();
        let portnum = MeshtasticLog::get_mesh_packet_portnum(&mesh_packet);
        if let Err(Error::EncryptionError { node_id, .. }) = portnum {
            debug!(
                "Ignoring encrypted message, cannot decrypt without a key. From node ID={node_id:x}."
            );
            return Ok(None);
        }
        let portnum = portnum?;
        match portnum {
            TELEMETRY_APP | POSITION_APP => self
                .msh_status_mesh_packet(mesh_packet, now, recv_mac_address)
                .map(|log| log.map(Event::MeshtasticLog)),
            SERIAL_APP => self
                .msh_serial_mesh_packet(mesh_packet, now)
                .map(|log| Some(Event::SiPunches(log))),
            _ => Ok(None),
        }
    }

    /// Parse the Status proto.
    ///
    /// # Aguments
    ///
    /// * `payload` - The serialized proto.
    /// * `mac_address` - The MAC address of the device the Status belongs to.
    /// * `now` - The timestamp when this proto was received.
    fn status_update(
        &mut self,
        payload: &[u8],
        mac_address: MacAddress,
        now: DateTime<FixedOffset>,
    ) -> crate::Result<CellularLogMessage> {
        let status_proto = Status::decode(payload).map_err(Error::FemtopbDecodeError)?;
        let log_message =
            CellularLogMessage::from_proto(status_proto, self.resolve(mac_address), now)?;

        let status = self.cellular_node_status(mac_address);
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

    /// Parse the Punches proto
    ///
    /// # Aguments
    ///
    /// * `payload` - The serialized proto.
    /// * `mac_address` - The MAC address of the device the Punches proto belongs to.
    /// * `now` - The timestamp when this proto was received.
    fn punches(
        &mut self,
        payload: &[u8],
        mac_address: MacAddress,
        now: DateTime<Local>,
    ) -> Result<Vec<SiPunchLog>, Error> {
        let now = now.into();
        let punches = Punches::decode(payload).map_err(Error::FemtopbDecodeError)?;
        let host_info = self.resolve(mac_address);
        let status = self.cellular_node_status(mac_address);
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

    fn msh_node_status(&mut self, host_info: &HostInfo) -> &mut MeshtasticNodeStatus {
        self.meshtastic_statuses.entry(host_info.mac_address).or_insert_with(|| {
            self.new_node.notify_one();
            MeshtasticNodeStatus::new(host_info.name.as_str().to_owned())
        })
    }

    /// Resolve a given MAC address into a full HostInfo, which also includes a name.
    fn resolve(&self, mac_address: MacAddress) -> HostInfo {
        let name = self.dns.get(&mac_address).map(|x| x.as_str()).unwrap_or("Unknown");
        HostInfo::new(name, mac_address)
    }

    /// Process Meshtastic status message given as MeshPacket.
    fn msh_status_mesh_packet(
        &mut self,
        mesh_packet: MeshPacket,
        now: DateTime<Local>,
        recv_mac_address: Option<MacAddress>,
    ) -> crate::Result<Option<MeshtasticLog>> {
        let recv_position = recv_mac_address.and_then(|mac_addr| self.get_position_name(mac_addr));
        let meshtastic_log =
            MeshtasticLog::from_mesh_packet(mesh_packet, now.into(), &self.dns, recv_position)?;
        self.msh_status_update(&meshtastic_log);
        Ok(meshtastic_log)
    }

    /// Process Meshtastic status message given as ServiceEnvelope.
    fn msh_status_service_envelope(
        &mut self,
        payload: &[u8],
        now: DateTime<Local>,
        recv_mac_address: MacAddress,
    ) -> crate::Result<Option<MeshtasticLog>> {
        let recv_position = self.get_position_name(recv_mac_address);
        let meshtastic_log =
            MeshtasticLog::from_service_envelope(payload, now.into(), &self.dns, recv_position)?;
        self.msh_status_update(&meshtastic_log);
        Ok(meshtastic_log)
    }

    fn msh_status_update(&mut self, log_message: &Option<MeshtasticLog>) {
        match log_message {
            Some(log_message) => {
                let status = self.msh_node_status(&log_message.host_info);
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
            _ => {
                trace!("Ignored meshtastic proto");
            }
        }
    }

    /// Process Meshtastic message of the serial module wrapped in ServiceEnvelope.
    fn msh_serial_service_envelope(
        &mut self,
        payload: &[u8],
        now: DateTime<Local>,
    ) -> crate::Result<Vec<SiPunchLog>> {
        let service_envelope = ServiceEnvelope::decode(payload)?;
        let packet = service_envelope.packet.ok_or(yaroc_common::error::Error::ValueError)?;
        self.msh_serial_mesh_packet(packet, now)
    }

    /// Process Meshtastic message of the serial module given as MeshPacket.
    pub fn msh_serial_mesh_packet(
        &mut self,
        packet: MeshPacket,
        now: DateTime<Local>,
    ) -> crate::Result<Vec<SiPunchLog>> {
        let mac_address = MacAddress::Meshtastic(packet.from);
        const SERIAL_APP: i32 = PortNum::SerialApp as i32;
        let host_info = self.resolve(mac_address);
        let Some(PayloadVariant::Decoded(Data {
            portnum: SERIAL_APP,
            payload,
            ..
        })) = packet.payload_variant
        else {
            // Encrypted message or wrong portnum (but wrong portnum should be filtered away)
            return Err(Error::EncryptionError {
                node_id: packet.from,
                channel_id: packet.channel,
            });
        };

        let status = self.msh_node_status(&host_info);
        let mut result = Vec::with_capacity(payload.len() / 20);

        let now = now.fixed_offset();
        let punches = yaroc_common::punch::SiPunch::punches_from_payload::<100>(
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

    /// Generate NodeInfo for all nodes.
    fn node_infos(&self) -> Vec<NodeInfo> {
        let mut res: Vec<_> = self
            .meshtastic_statuses
            .values()
            .map(|status| status.serialize())
            .chain(self.cellular_statuses.values().map(|status| status.serialize()))
            .collect();
        res.sort_by(|a, b| a.name.cmp(&b.name));
        res
    }

    pub async fn publish_node_infos(&mut self) -> Vec<NodeInfo> {
        let next_node_infos = self.last_node_info_push + self.node_infos_interval;
        tokio::select! {
            _ = tokio::time::sleep_until(next_node_infos.into()) => {}
            _ = self.new_node.notified() => {
                info!("New node discovered!");
            }
        }
        self.last_node_info_push = Instant::now();
        self.node_infos()
    }

    fn get_position_name(&self, mac_address: MacAddress) -> Option<PositionName> {
        let status = self.meshtastic_statuses.get(&mac_address)?;
        status
            .position
            .as_ref()
            .map(|position| PositionName::new(position, &status.name))
    }

    fn cellular_node_status(&mut self, mac_addr: MacAddress) -> &mut CellularNodeStatus {
        let host_info = self.resolve(mac_addr);
        self.cellular_statuses.entry(mac_addr).or_insert_with(|| {
            self.new_node.notify_one();
            CellularNodeStatus::new(host_info)
        })
    }
}

#[cfg(test)]
mod test_punch {
    use super::*;

    use yaroc_common::proto::status::Msg;
    use yaroc_common::proto::{MiniCallHome, Punch, Timestamp};
    use yaroc_common::punch::SiPunch;

    use chrono::Local;
    use femtopb::{EnumValue, Repeated};

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
        let mut buf = vec![0u8; punches.encoded_len()];
        punches.encode(&mut buf.as_mut_slice()).unwrap();

        let mut state = FleetState::default();
        let now = Local::now();
        // TODO: should propagate errors
        let punches = state.punches(&buf, MacAddress::default(), now).unwrap();
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
        let mut buf = vec![0u8; punches.encoded_len()];
        punches.encode(&mut buf.as_mut_slice()).unwrap();

        let mut state = FleetState::default();
        let now = Local::now();
        let punch_logs = state.punches(&buf, MacAddress::default(), now).unwrap();
        assert_eq!(punch_logs.len(), 1);
        assert_eq!(punch_logs[0].punch.code, 47);
        assert_eq!(punch_logs[0].punch.card, 1715004);
    }

    #[test]
    fn test_cellular_status() {
        let timestamp = Timestamp {
            millis_epoch: 1706523131_124, // 2024-01-29T11:12:11.124+01:00
            ..Default::default()
        };
        let status = Status {
            msg: Some(Msg::MiniCallHome(MiniCallHome {
                cpu_temperature: 47.0,
                millivolts: 3847,
                network_type: EnumValue::Known(yaroc_common::proto::CellNetworkType::LteM),
                signal_dbm: -80,
                signal_snr_cb: 120,
                time: Some(timestamp),
                ..Default::default()
            })),
            ..Default::default()
        };
        let mut state = FleetState::new(
            vec![("spe01".to_owned(), MacAddress::default())],
            Duration::from_secs(1),
        );
        let mut buffer = vec![0u8; status.encoded_len()];
        status.encode(&mut buffer.as_mut_slice()).unwrap();
        let tz = FixedOffset::east_opt(3600).unwrap();
        let now = Local::now().with_timezone(&tz);
        let log_message = state.status_update(&buffer, MacAddress::default(), now).unwrap();
        assert!(
            format!("{log_message}")
                .starts_with("spe01 11:12:11: 47.0°C, RSSI  -80 SNR 12.0   LTE-M, 3.85V")
        );
    }
}

#[cfg(test)]
mod test_meshtastic {
    use super::*;

    use crate::meshtastic::RssiSnr;
    use meshtastic::protobufs::telemetry::Variant;
    use meshtastic::protobufs::{DeviceMetrics, ServiceEnvelope, Telemetry};

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
        let punch = yaroc_common::punch::SiPunch::new(1715004, 47, time, 2).raw;

        let message = envelope(
            0xdeadbeef,
            Data {
                portnum: PortNum::SerialApp as i32,
                payload: punch.to_vec(),
                ..Default::default()
            },
        )
        .encode_to_vec();
        let mut state = FleetState::default();
        let punch_logs = state.msh_serial_service_envelope(&message, Local::now()).unwrap();
        assert_eq!(punch_logs.len(), 1);
        assert_eq!(punch_logs[0].punch.code, 47);
        assert_eq!(punch_logs[0].punch.card, 1715004);
        let node_infos = state.node_infos();
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
        let mut state = FleetState::default();
        state
            .msh_status_service_envelope(&message, Local::now(), MacAddress::default())
            .unwrap();
        let node_infos = state.node_infos();
        assert_eq!(node_infos.len(), 1);
        assert_eq!(
            node_infos[0].signal_info,
            SignalInfo::Meshtastic(RssiSnr {
                rssi_dbm: -98,
                snr: 4.0,
                distance: None
            })
        );

        let mesh_packet = MeshPacket {
            payload_variant: Some(PayloadVariant::Decoded(data)),
            ..Default::default()
        };
        state
            .msh_status_mesh_packet(mesh_packet, Local::now(), Some(MacAddress::default()))
            .unwrap();
        let node_infos = state.node_infos();
        assert_eq!(node_infos.len(), 1);
        assert_eq!(node_infos[0].signal_info, SignalInfo::Unknown);
    }

    #[test]
    fn test_meshtastic_serial_and_status() {
        let time = DateTime::parse_from_rfc3339("2023-11-23T10:00:03+01:00").unwrap();
        let punch = yaroc_common::punch::SiPunch::new(1715004, 47, time, 2).raw;

        let message = envelope(
            0xdeadbeef,
            Data {
                portnum: PortNum::SerialApp as i32,
                payload: punch.to_vec(),
                ..Default::default()
            },
        )
        .encode_to_vec();

        let mut state = FleetState::default();
        state.msh_serial_service_envelope(&message, Local::now()).unwrap();

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
        state
            .msh_status_service_envelope(&message, Local::now(), MacAddress::default())
            .unwrap();
        let node_infos = state.node_infos();
        assert_eq!(node_infos.len(), 1);
    }
}
