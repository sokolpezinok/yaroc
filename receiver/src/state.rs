use chrono::DateTime;
use chrono::prelude::*;
use femtopb::Message as _;
use futures::StreamExt;
use log::{debug, error, info, trace, warn};
use meshtastic::protobufs::MeshPacket;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tokio_util::time::DelayQueue;
use tokio_util::time::delay_queue::Key;

use crate::error::Error;
use crate::logs::{CellularLogMessage, SiPunchLog};
use crate::meshtastic::{
    MeshtasticLog, MshMetrics, POSITION_APP, PositionName, RssiSnr, SERIAL_APP, TELEMETRY_APP,
    punches_from_mesh_packet, unpack_envelope,
};
use crate::mqtt::Message as MqttMessage;
use crate::system_info::{HostInfo, MacAddress};
use yaroc_common::proto::{Punches, Status};
use yaroc_common::punch::SiPunch;
use yaroc_common::status::{CellSignalInfo, SignalStrength, voltage_to_percent};

/// Connection type and signal details of a fleet node.
#[derive(Clone, Debug, PartialEq)]
pub enum SignalInfo {
    /// The connection state or signal strength is unknown.
    Unknown,
    /// Connected via cellular network.
    Cell(CellSignalInfo),
    /// Connected via Meshtastic network.
    Meshtastic(RssiSnr),
    /// Connected via a USB cable or over MQTT. No LoRa transmission involved.
    MeshtasticOverWire,
}

impl SignalInfo {
    /// Returns the [`SignalStrength`] corresponding to this connection type.
    pub fn signal_strength(&self) -> SignalStrength {
        match self {
            SignalInfo::Cell(cell_signal_info) => cell_signal_info.signal_strength(),
            SignalInfo::Meshtastic(rssi_snr) => rssi_snr.signal_strength(),
            SignalInfo::MeshtasticOverWire => SignalStrength::Excellent,
            SignalInfo::Unknown => SignalStrength::Disconnected,
        }
    }
}

/// A representation of a node's current status and diagnostic state.
#[derive(Clone, Debug)]
pub struct NodeInfo {
    /// The name of the node.
    pub name: String,
    /// The signal strength and connection type.
    pub signal_info: SignalInfo,
    /// Battery level in percent, if available.
    pub battery_percentage: Option<u8>,
    /// List of punch codes (station IDs) collected by this node.
    pub codes: Vec<u16>,
    /// The timestamp of the last received status update.
    pub last_update: Option<DateTime<FixedOffset>>,
    /// The timestamp of the last recorded punch.
    pub last_punch: Option<DateTime<FixedOffset>>,
}

/// Tracks the diagnostic status and accumulated station codes of a cellular node.
#[derive(Default, Clone)]
pub struct CellularNodeStatus {
    /// Information about the host device (MAC, name).
    host_info: HostInfo,
    /// Detailed cellular signal information, or `None` if disconnected.
    state: Option<CellSignalInfo>,
    /// Current battery level of the node in percent.
    battery_percentage: Option<u8>,
    /// Set of station codes that have been punched through this node.
    codes: HashSet<u16>,
    /// Timestamp of the last received status update.
    last_update: Option<DateTime<FixedOffset>>,
    /// Timestamp of the last recorded punch.
    last_punch: Option<DateTime<FixedOffset>>,
}

impl CellularNodeStatus {
    /// Creates a new, default status tracker for the given cellular host.
    pub fn new(host_info: HostInfo) -> Self {
        Self {
            host_info,
            ..Self::default()
        }
    }

    /// Simulates or updates a disconnection event by clearing signal state.
    pub fn disconnect(&mut self) {
        self.state = None;
        self.last_update = Some(Local::now().into());
    }

    /// Translates raw battery voltage into a percentage and stores it.
    pub fn update_voltage(&mut self, mv: u16) {
        let percent = voltage_to_percent(mv);
        self.battery_percentage = Some(percent);
    }

    /// Updates the signal state and update timestamp upon a connection event.
    pub fn mqtt_connect_update(&mut self, signal_info: CellSignalInfo) {
        self.state = Some(signal_info);
        self.last_update = Some(Local::now().into());
    }

    /// Updates punch-related metrics, storing the punch time and recording the code.
    pub fn punch(&mut self, punch: &SiPunch) {
        self.last_punch = Some(punch.time);
        self.codes.insert(punch.code);
    }

    /// Converts the cellular node status into a standard, serializable [`NodeInfo`].
    pub fn serialize(&self) -> NodeInfo {
        let signal_info = match self.state {
            Some(signal_info) => SignalInfo::Cell(signal_info),
            None => SignalInfo::Unknown,
        };

        NodeInfo {
            name: self.host_info.name.to_string(),
            signal_info,
            battery_percentage: self.battery_percentage,
            codes: self.codes.iter().copied().collect(),
            last_update: self.last_update,
            last_punch: self.last_punch,
        }
    }

    /// Returns the signal strength of the cellular connection.
    pub fn signal_strength(&self) -> SignalStrength {
        match self.state {
            Some(cell_signal_info) => cell_signal_info.signal_strength(),
            None => SignalStrength::Disconnected,
        }
    }
}

/// Tracks the diagnostic status and accumulated station codes of a Meshtastic node.
#[derive(Default, Clone)]
pub struct MeshtasticNodeStatus {
    /// The hostname or identity of the Meshtastic device.
    pub name: String,
    /// Battery level in percent, if available.
    battery_percentage: Option<u8>,
    /// The RSSI and SNR connection metrics of the last transmission.
    pub rssi_snr: Option<RssiSnr>,
    /// Geographical coordinates and altitude of the node.
    pub position: Option<yaroc_common::status::Position>,
    /// Station codes that have been transmitted through this node.
    codes: HashSet<u16>,
    /// The timestamp of the last status update received.
    last_update: Option<DateTime<FixedOffset>>,
    /// The timestamp of the last punch recorded on this node.
    last_punch: Option<DateTime<FixedOffset>>,
    /// Indicates whether the node is currently considered active.
    connected: bool,
    /// Internal tracking key for handling network timeouts.
    pub timeout_key: Option<Key>,
}

impl MeshtasticNodeStatus {
    /// Creates a new status tracker for a Meshtastic node.
    pub fn new(name: String) -> Self {
        Self {
            name,
            connected: true,
            timeout_key: None,
            ..Default::default()
        }
    }

    /// Marks the node as disconnected.
    pub fn disconnect(&mut self) {
        self.connected = false;
    }

    /// Updates the node's battery percentage and updates the last status timestamp.
    pub fn update_battery(&mut self, percent: u8) {
        self.battery_percentage = Some(percent);
        self.last_update = Some(Local::now().into());
    }

    /// Updates the RSSI and SNR details, marking the node as connected.
    pub fn update_rssi_snr(&mut self, rssi_snr: RssiSnr) {
        self.connected = true;
        self.rssi_snr = Some(rssi_snr);
        self.last_update = Some(Local::now().into());
    }

    /// Clears the RSSI and SNR state, for instance, on link degradation.
    pub fn clear_rssi_snr(&mut self) {
        self.rssi_snr = None;
        self.last_update = Some(Local::now().into());
    }

    /// Registers a punch event, updating the last punch timestamp and adding the station code.
    pub fn punch(&mut self, punch: &SiPunch) {
        self.connected = true;
        self.last_punch = Some(punch.time);
        self.codes.insert(punch.code);
    }

    /// Serializes this Meshtastic node status into a standard [`NodeInfo`].
    pub fn serialize(&self) -> NodeInfo {
        let signal_info = if !self.connected {
            SignalInfo::Unknown
        } else {
            match &self.rssi_snr {
                Some(rssi_snr) => SignalInfo::Meshtastic(rssi_snr.clone()),
                None => SignalInfo::MeshtasticOverWire,
            }
        };
        NodeInfo {
            name: self.name.clone(),
            signal_info,
            battery_percentage: self.battery_percentage,
            codes: self.codes.iter().copied().collect(),
            last_update: self.last_update,
            last_punch: self.last_punch,
        }
    }
}

/// Events produced by the receiver's state machine for routing and reporting.
#[derive(Debug)]
pub enum Event {
    /// A log or status message from a cellular node.
    CellularLog(CellularLogMessage),
    /// A collection of processed SportIdent punch logs.
    SiPunches(Vec<SiPunchLog>),
    /// A single SportIdent punch event.
    SiPunch(SiPunch),
    /// A telemetry or status update from a Meshtastic node.
    MeshtasticLog(MeshtasticLog),
    /// A collective update of the statuses of all nodes.
    NodeInfos(Vec<NodeInfo>),
    /// A local serial or USB device change event.
    DeviceEvent {
        /// True if the device was connected; false if disconnected.
        added: bool,
        /// The serial or identifier string of the device.
        device: String,
    },
}

/// Tracks the statuses, connection states, and data collections of the whole node fleet.
///
/// It coordinates processing of messages from both cellular and Meshtastic nodes,
/// handles timeouts, and schedules periodic node information updates.
pub struct FleetState {
    /// A simple DNS-like map associating node MAC addresses with user-friendly names.
    dns: HashMap<MacAddress, String>,
    /// State of all cellular nodes.
    cellular_statuses: HashMap<MacAddress, CellularNodeStatus>,
    /// State of all Meshtastic nodes.
    meshtastic_statuses: HashMap<MacAddress, MeshtasticNodeStatus>,
    /// Tracks timeouts of Meshtastic nodes to detect offline status.
    meshtastic_timeouts: DelayQueue<MacAddress>,
    /// The minimum interval between node info broadcasts.
    node_infos_interval: Duration,
    /// The duration since the last activity before a Meshtastic node is considered offline.
    meshtastic_timeout: Duration,
    /// Timestamp when the node information list was last generated/sent.
    last_node_info_push: Instant,
    /// Signal notifier triggered when a previously unseen node joins the network.
    new_node: Notify,
}

impl Default for FleetState {
    /// Creates a default empty [`FleetState`] tracker.
    fn default() -> Self {
        Self {
            dns: HashMap::new(),
            cellular_statuses: HashMap::new(),
            meshtastic_statuses: HashMap::new(),
            meshtastic_timeouts: DelayQueue::new(),
            node_infos_interval: Duration::from_secs(60),
            meshtastic_timeout: Duration::from_secs(600),
            last_node_info_push: Instant::now(),
            new_node: Notify::new(),
        }
    }
}

impl FleetState {
    /// Creates a new [`FleetState`] tracker.
    ///
    /// # Arguments
    ///
    /// * `dns` - Mapping of device friendly names to MAC addresses.
    /// * `node_infos_interval` - Time interval for periodic status updates.
    /// * `meshtastic_timeout` - Inactivity timeout limit for Meshtastic nodes.
    pub fn new(
        dns: Vec<(String, MacAddress)>,
        node_infos_interval: Duration,
        meshtastic_timeout: Duration,
    ) -> Self {
        Self {
            dns: dns.into_iter().map(|(name, mac)| (mac, name)).collect(),
            node_infos_interval,
            meshtastic_timeout,
            ..Default::default()
        }
    }

    /// Processes an incoming MQTT message and returns a parsed system event, if applicable.
    ///
    /// # Arguments
    ///
    /// * `mqtt_message` - The MQTT message wrapper.
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

    /// Processes a `MeshPacket` received from a Meshtastic mesh node.
    ///
    /// # Arguments
    ///
    /// * `mesh_packet` - The raw protobuf packet from the mesh network.
    /// * `recv_mac_address` - The local receiver node's MAC address.
    pub fn process_mesh_packet(
        &mut self,
        mesh_packet: MeshPacket,
        recv_mac_address: MacAddress,
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
                .map(|punches| Some(Event::SiPunches(punches))),
            _ => Ok(None),
        }
    }

    /// Parses cellular Status messages, updating target connection and power levels.
    ///
    /// # Arguments
    ///
    /// * `payload` - Raw protobuf payload of the Status.
    /// * `mac_address` - MAC address of the source device.
    /// * `now` - Server receive time.
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
                    status.update_voltage(batt_mv);
                }
            }
            CellularLogMessage::Disconnected { .. } => {
                status.disconnect();
            }
            _ => {}
        }
        Ok(log_message)
    }

    /// Parses cellular Punches messages, updating local punch caches and status.
    ///
    /// # Arguments
    ///
    /// * `payload` - Raw protobuf payload of the punches.
    /// * `mac_address` - MAC address of the source device.
    /// * `now` - Current system time when processed.
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
            let si_punch_log = SiPunchLog::from_bytes(punch, host_info.clone(), now);
            if let Some((si_punch_log, rest)) = si_punch_log {
                status.punch(&si_punch_log.punch);
                result.push(si_punch_log);
                if !rest.is_empty() {
                    warn!("Residual bytes after parsing punch: {rest:?}");
                }
            } else {
                error!("Wrong punch format: {:?}", punch);
            }
        }

        Ok(result)
    }

    /// Retrieves or creates a status tracker for a given Meshtastic host, resetting its timeout.
    fn msh_node_status(&mut self, host_info: &HostInfo) -> &mut MeshtasticNodeStatus {
        let mac_addr = host_info.mac_address;
        let mut is_new = false;
        let status = self.meshtastic_statuses.entry(mac_addr).or_insert_with(|| {
            is_new = true;
            MeshtasticNodeStatus::new(host_info.name.to_string())
        });
        if is_new {
            self.new_node.notify_one();
        }
        if let Some(key) = &status.timeout_key {
            self.meshtastic_timeouts.reset(key, self.meshtastic_timeout);
        } else {
            let key = self.meshtastic_timeouts.insert(mac_addr, self.meshtastic_timeout);
            status.timeout_key = Some(key);
        }
        status
    }

    /// Resolves a given MAC address into a full [`HostInfo`] structure containing its name.
    fn resolve(&self, mac_address: MacAddress) -> HostInfo {
        let name = self.dns.get(&mac_address).map(|x| x.as_str()).unwrap_or("Unknown");
        HostInfo::new(name, mac_address)
    }

    /// Processes a Meshtastic status update given as a `MeshPacket`.
    fn msh_status_mesh_packet(
        &mut self,
        mesh_packet: MeshPacket,
        now: DateTime<Local>,
        recv_mac_address: MacAddress,
    ) -> crate::Result<Option<MeshtasticLog>> {
        let recv_position = self.get_position_name(recv_mac_address);
        let meshtastic_log =
            MeshtasticLog::from_mesh_packet(mesh_packet, now.into(), &self.dns, recv_position)?;
        self.msh_status_update(&meshtastic_log);
        Ok(meshtastic_log)
    }

    /// Processes a Meshtastic status update given as a `ServiceEnvelope`.
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

    /// Updates internal metrics for a Meshtastic node from a parsed log message.
    fn msh_status_update(&mut self, log_message: &Option<MeshtasticLog>) {
        match log_message {
            Some(log_message) => {
                let status = self.msh_node_status(&log_message.host_info);
                match log_message.metrics {
                    MshMetrics::Battery { percent, .. } => {
                        status.update_battery(percent as u8);
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

    /// Processes a serial module message from Meshtastic wrapped in a `ServiceEnvelope`.
    fn msh_serial_service_envelope(
        &mut self,
        payload: &[u8],
        now: DateTime<Local>,
    ) -> crate::Result<Vec<SiPunchLog>> {
        let packet = unpack_envelope(payload)?;
        self.msh_serial_mesh_packet(packet, now)
    }

    /// Processes a serial module message from Meshtastic given as a `MeshPacket`.
    pub fn msh_serial_mesh_packet(
        &mut self,
        packet: MeshPacket,
        now: DateTime<Local>,
    ) -> crate::Result<Vec<SiPunchLog>> {
        let mac_address = MacAddress::Meshtastic(packet.from);
        let rssi_snr = RssiSnr::from_mesh_packet(&packet);
        let punches = punches_from_mesh_packet(packet, now.into(), &self.dns)?;

        let host_info = self.resolve(mac_address);
        let status = self.msh_node_status(&host_info);
        for punch_log in &punches {
            status.punch(&punch_log.punch);
        }
        if let Some(rssi_snr) = rssi_snr {
            status.update_rssi_snr(rssi_snr);
        }
        Ok(punches)
    }

    /// Generates and sorts the `NodeInfo` summaries for all registered nodes.
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

    /// Async loop selection that publishes node info reports when the interval expires,
    /// a new node joins, or a Meshtastic node times out.
    pub async fn publish_node_infos(&mut self) -> Vec<NodeInfo> {
        let next_node_infos = self.last_node_info_push + self.node_infos_interval;
        tokio::select! {
            _ = tokio::time::sleep_until(next_node_infos.into()) => {}
            _ = self.new_node.notified() => {
                info!("New node discovered!");
            }
            Some(expired) = self.meshtastic_timeouts.next(), if !self.meshtastic_timeouts.is_empty() => {
                let mac = expired.into_inner();
                if let Some(status) = self.meshtastic_statuses.get_mut(&mac) {
                    status.disconnect();
                    status.timeout_key = None;
                    info!("Meshtastic node {} timed out", status.name);
                }
            }
        }
        self.last_node_info_push = Instant::now();
        self.node_infos()
    }

    /// Resolves the name of a given MAC address's position name, if tracking GPS.
    fn get_position_name(&self, mac_address: MacAddress) -> Option<PositionName> {
        let status = self.meshtastic_statuses.get(&mac_address)?;
        status
            .position
            .as_ref()
            .map(|position| PositionName::new(position, &status.name))
    }

    /// Retrieves or inserts a cellular status tracker for the given MAC address.
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
    use yaroc_common::proto::{MiniCallHome, Timestamp};
    use yaroc_common::punch::SiPunch;

    use chrono::Local;
    use femtopb::{EnumValue, Repeated};

    #[test]
    fn test_wrong_punch() {
        let punches_slice: &[&[u8]] = &[b"\x12\x43\xa7"];
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
        let punch = SiPunch::new_send_last_record(1715004, 47, time, 2).raw;
        let punches_slice: &[&[u8]] = &[&punch];
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
            millis_epoch: 1_706_523_131_124, // 2024-01-29T11:12:11.124+01:00
            ..Default::default()
        };
        let status = Status {
            msg: Some(Msg::MiniCallHome(MiniCallHome {
                cpu_temperature: 47.0,
                millivolts: 3847,
                network_type: EnumValue::Known(yaroc_common::proto::CellNetworkType::LteM),
                rsrp_dbm: -80,
                signal_snr_cb: 120,
                time: Some(timestamp),
                ..Default::default()
            })),
            ..Default::default()
        };
        let mut state = FleetState::new(
            vec![("spe01".to_owned(), MacAddress::default())],
            Duration::from_secs(1),
            Duration::from_secs(600),
        );
        let mut buffer = vec![0u8; status.encoded_len()];
        status.encode(&mut buffer.as_mut_slice()).unwrap();
        let tz = FixedOffset::east_opt(3600).unwrap();
        let now = Local::now().with_timezone(&tz);
        let log_message = state.status_update(&buffer, MacAddress::default(), now).unwrap();
        assert!(
            format!("{log_message}")
                .starts_with("spe01 11:12:11: 47.0°C, RSRP  -80 SNR 12.0   LTE-M, 3.85V")
        );

        let node_infos = state.node_infos();
        assert_eq!(node_infos.len(), 1);
        assert_eq!(
            node_infos[0].battery_percentage,
            Some(voltage_to_percent(3847))
        );
        std::assert_matches!(node_infos[0].signal_info, SignalInfo::Cell(_));
    }

    #[test]
    fn test_cellular_disconnect() {
        use yaroc_common::proto::Disconnected;
        let status = Status {
            msg: Some(Msg::Disconnected(Disconnected {
                client_name: "spe01",
                ..Default::default()
            })),
            ..Default::default()
        };
        let mut state = FleetState::new(
            vec![("spe01".to_owned(), MacAddress::default())],
            Duration::from_secs(1),
            Duration::from_secs(600),
        );
        // Pretend we were connected first
        let status_node = state.cellular_node_status(MacAddress::default());
        status_node.state = Some(yaroc_common::status::CellSignalInfo {
            network_type: yaroc_common::status::CellNetworkType::Lte,
            rsrp_dbm: -90,
            snr_cb: 110,
            cellid: None,
        });

        let mut buffer = vec![0u8; status.encoded_len()];
        status.encode(&mut buffer.as_mut_slice()).unwrap();
        let tz = FixedOffset::east_opt(3600).unwrap();
        let now = Local::now().with_timezone(&tz);
        let log_message = state.status_update(&buffer, MacAddress::default(), now).unwrap();
        std::assert_matches!(log_message, CellularLogMessage::Disconnected { .. });
        let node_infos = state.node_infos();
        assert_eq!(node_infos.len(), 1);
        assert_eq!(node_infos[0].signal_info, SignalInfo::Unknown);
    }
}

#[cfg(test)]
mod test_meshtastic {
    use super::*;

    use crate::meshtastic::RssiSnr;
    use meshtastic::Message;
    use meshtastic::protobufs::mesh_packet::PayloadVariant;
    use meshtastic::protobufs::telemetry::Variant;
    use meshtastic::protobufs::{Data, DeviceMetrics, PortNum, ServiceEnvelope, Telemetry};

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

    #[tokio::test]
    async fn test_meshtastic_serial() {
        let time = DateTime::parse_from_rfc3339("2023-11-23T10:00:03+01:00").unwrap();
        let punch = SiPunch::new_send_last_record(1715004, 47, time, 2).raw;

        let mut env = envelope(
            0xdeadbeef,
            Data {
                portnum: PortNum::SerialApp as i32,
                payload: punch.to_vec(),
                ..Default::default()
            },
        );
        env.packet.as_mut().unwrap().rx_rssi = -90;
        env.packet.as_mut().unwrap().rx_snr = 4.5;
        let message = env.encode_to_vec();

        let mut state = FleetState::default();
        let punch_logs = state.msh_serial_service_envelope(&message, Local::now()).unwrap();
        assert_eq!(punch_logs.len(), 1);
        assert_eq!(punch_logs[0].punch.code, 47);
        assert_eq!(punch_logs[0].punch.card, 1715004);
        let rssi_snr = punch_logs[0].rssi_snr.clone().unwrap();
        assert_eq!(rssi_snr.rssi_dbm, -90);
        assert_eq!(rssi_snr.snr, 4.5);

        let node_infos = state.node_infos();
        assert_eq!(node_infos.len(), 1);
        assert_eq!(
            node_infos[0].last_punch.unwrap().time(),
            NaiveTime::from_hms_opt(10, 0, 3).unwrap()
        );
        assert_eq!(
            node_infos[0].signal_info,
            SignalInfo::Meshtastic(RssiSnr {
                rssi_dbm: -90,
                snr: 4.5,
                hop_count: 0,
                distance: None
            })
        );
    }

    #[tokio::test]
    async fn test_meshtastic_status() {
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
        assert_eq!(node_infos[0].battery_percentage, Some(47));
        assert_eq!(
            node_infos[0].signal_info,
            SignalInfo::Meshtastic(RssiSnr {
                rssi_dbm: -98,
                snr: 4.0,
                hop_count: 0,
                distance: None
            })
        );

        let mesh_packet = MeshPacket {
            payload_variant: Some(PayloadVariant::Decoded(data)),
            ..Default::default()
        };
        state
            .msh_status_mesh_packet(mesh_packet, Local::now(), MacAddress::default())
            .unwrap();
        let node_infos = state.node_infos();
        assert_eq!(node_infos.len(), 1);
        assert_eq!(node_infos[0].signal_info, SignalInfo::MeshtasticOverWire);
        assert_eq!(
            node_infos[0].signal_info.signal_strength(),
            SignalStrength::Excellent
        );
    }

    #[tokio::test]
    async fn test_meshtastic_serial_and_status() {
        let time = DateTime::parse_from_rfc3339("2023-11-23T10:00:03+01:00").unwrap();
        let punch = SiPunch::new_send_last_record(1715004, 47, time, 2).raw;

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

    #[test]
    fn test_cellular_node_signal_strength() {
        use yaroc_common::status::{CellNetworkType, CellSignalInfo};
        let mut status = CellularNodeStatus::default();
        assert_eq!(status.signal_strength(), SignalStrength::Disconnected);

        status.state = Some(CellSignalInfo {
            network_type: CellNetworkType::Lte,
            rsrp_dbm: -90,
            snr_cb: 110,
            cellid: None,
        });
        assert_eq!(status.signal_strength(), SignalStrength::Excellent);
    }

    #[tokio::test]
    async fn test_meshtastic_disconnect() {
        let mut state = FleetState::default();
        let host_info = HostInfo::new("msh_node", MacAddress::default());
        let status = state.msh_node_status(&host_info);

        status.update_rssi_snr(RssiSnr::new(-90, 4.0, 0).unwrap());

        let node_info = status.serialize();
        assert!(matches!(node_info.signal_info, SignalInfo::Meshtastic(_)));
        assert!(node_info.last_update.is_some());

        let last_update = node_info.last_update;
        // Disconnect and ensure connected is false and last_update does not change
        status.disconnect();
        let node_info_disconnected = status.serialize();
        assert_eq!(node_info_disconnected.signal_info, SignalInfo::Unknown);
        assert_eq!(node_info_disconnected.last_update, last_update);

        // Ensure punch reconnects the node
        let time = DateTime::parse_from_rfc3339("2023-11-23T10:00:03+01:00").unwrap();
        let punch = SiPunch::new_send_last_record(1715004, 47, time, 2);
        status.punch(&punch);
        let node_info_reconnected = status.serialize();
        assert!(matches!(
            node_info_reconnected.signal_info,
            SignalInfo::Meshtastic(_)
        ));
    }

    #[tokio::test]
    async fn test_meshtastic_timeout() {
        let mut state = FleetState {
            meshtastic_timeout: Duration::from_millis(100),
            ..Default::default()
        };
        let host_info = HostInfo::new("msh_node", MacAddress::default());

        // This will insert the node and set the timeout key
        let status = state.msh_node_status(&host_info);
        status.update_rssi_snr(RssiSnr::new(-90, 4.0, 0).unwrap());

        let node_info = status.serialize();
        assert!(matches!(node_info.signal_info, SignalInfo::Meshtastic(_)));

        // Consume the new_node notification
        state.publish_node_infos().await;

        // Wait for the timeout to expire
        tokio::time::sleep(Duration::from_millis(150)).await;

        // publish_node_infos will process the timeout
        let node_infos = state.publish_node_infos().await;

        assert_eq!(node_infos.len(), 1);
        assert_eq!(node_infos[0].signal_info, SignalInfo::Unknown);
    }
}
