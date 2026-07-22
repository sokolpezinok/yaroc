use chrono::{FixedOffset, Local};
use log::{debug, error, info};
use meshtastic::protobufs::mesh_packet::PayloadVariant;
use meshtastic::protobufs::{MeshPacket, ServiceEnvelope};
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinSet;

use yaroc_common::punch::SiPunch;

use crate::meshtastic::serial::MeshtasticFactory;
use crate::meshtastic::tcp;
use crate::si_uart::{SportIdentFactory, SportIdentMessage};
use crate::usb_serial_manager::{UsbSerialFactory, UsbSerialManager};
use crate::{
    mqtt::{Message, MqttConfig, MqttReceiver},
    state::{Event, FleetState},
    system_info::MacAddress,
};

pub struct MessageHandlerInitializer {
    pub meshtastic_tcp: Option<String>,
    pub mqtt_receivers: Vec<MqttReceiver>,
    pub fake_punch_config: Option<FakePunchConfig>,
    pub usb_serial_manager: Option<UsbSerialManager>,
}

/// Orchestrates the overall message flow.
///
/// This struct is responsible for receiving messages from MQTT, SportIdent and Meshtastic devices,
/// processing them, and maintaining the state of the fleet.
pub struct MessageHandler {
    fleet_state: FleetState,
    mesh_packet_rx: UnboundedReceiver<ServiceEnvelope>,
    _mesh_packet_tx: UnboundedSender<ServiceEnvelope>, // Kept to prevent channel from closing
    si_rx: UnboundedReceiver<SportIdentMessage>,
    si_tx: UnboundedSender<SportIdentMessage>, // Kept to prevent channel from closing
    /// TX side of inbound MQTT messages
    inbound_mqtt_tx: UnboundedSender<crate::Result<Message>>,
    /// RX side of inbound MQTT messages
    inbound_mqtt_rx: UnboundedReceiver<crate::Result<Message>>,
    /// Senders for meshtastic messages
    mesh_txs: Vec<UnboundedSender<ServiceEnvelope>>,
    tasks: JoinSet<()>,
    initializer: Option<MessageHandlerInitializer>,
    timezone: FixedOffset,
    local_macs: HashSet<MacAddress>,
}

#[derive(Default)]
pub enum SportIdentConfig {
    #[default]
    None,
    Passive,
    Active(Box<dyn UsbSerialFactory>),
}

#[derive(Default)]
pub struct UsbSerialConfig {
    pub enable_meshtastic: bool,
    pub sportident: SportIdentConfig,
}

impl MessageHandler {
    /// Spawn async background tasks when Self:;next_message() is ran for the first time.
    pub async fn init(&mut self) {
        if let Some(init) = self.initializer.take() {
            if let Some(host) = init.meshtastic_tcp {
                let mesh_packet_tx = self._mesh_packet_tx.clone();
                self.tasks.spawn(async move {
                    tcp::connect_and_loop(host, mesh_packet_tx).await;
                });
            }

            if let Some(fake_punch_config) = init.fake_punch_config {
                let si_tx = self.si_tx.clone();
                let card = fake_punch_config.card;
                let code = fake_punch_config.code;
                let interval = fake_punch_config.interval;
                info!(
                    "Starting a fake SportIdent worker, sending a punch every {interval:?} with card {card} and code {code}"
                );
                let timezone = self.timezone;
                self.tasks.spawn(async move {
                    let mut interval_timer = tokio::time::interval(interval);
                    // The first tick of tokio::time::interval completes immediately.
                    loop {
                        interval_timer.tick().await;
                        let now = Local::now().with_timezone(&timezone);
                        let punch = SiPunch::new_send_last_record(card, code, now, 18);
                        if let Err(e) = si_tx.send(SportIdentMessage::RawPunch(punch.raw)) {
                            error!("Failed to send fake punch: {e}");
                            break;
                        }
                    }
                });
            }

            for mut receiver in init.mqtt_receivers {
                self.mesh_txs.push(receiver.mesh_tx());
                let mqtt_tx = self.inbound_mqtt_tx.clone();
                self.tasks.spawn(async move {
                    loop {
                        let msg = receiver.next_message().await;
                        let res =
                            mqtt_tx.send(msg).map_err(|_| crate::error::Error::ChannelSendError);
                        if let Err(e) = res {
                            error!(
                                "Error while receiving and forwarding MQTT message from {}: {e}",
                                receiver.url()
                            );
                        }
                    }
                });
            }

            if let Some(mut usb_manager) = init.usb_serial_manager {
                self.tasks.spawn(async move {
                    if let Err(e) = usb_manager.monitor_usb_devices().await {
                        error!("USB device manager failed: {e}");
                    }
                });
            }
        }
    }

    /// Process ServiceEnvelope proto
    fn process_service_envelope(
        &mut self,
        service_envelope: ServiceEnvelope,
    ) -> crate::Result<Option<Event>> {
        let now = Local::now().with_timezone(&self.timezone);
        let gateway_id = service_envelope
            .gateway_id
            .strip_prefix('!')
            .unwrap_or(&service_envelope.gateway_id);
        let mac_address = MacAddress::try_from(gateway_id)?;
        self.local_macs.insert(mac_address);
        for mesh_tx in &self.mesh_txs {
            let _ = mesh_tx
                .send(service_envelope.clone())
                .inspect_err(|e| error!("Error while forwarding meshtastic packets: {}", e));
        }
        self.fleet_state.process_mesh_packet(service_envelope, mac_address, now)
    }

    /// Returns the next processed event from the active event sources.
    ///
    /// This function asynchronously polls multiple message streams (MQTT background tasks, Meshtastic
    /// packet receiver channel, SportIdent punch receiver channel, and node info publication interval)
    /// using `tokio::select!`. It returns the first successfully decoded and processed `Event`.
    ///
    /// # Errors
    ///
    /// Returns an error if any of the underlying event parsing or state updates fail.
    pub async fn next_event(&mut self) -> crate::Result<Event> {
        self.init().await;
        loop {
            tokio::select! {
                Some(mqtt_message) = self.inbound_mqtt_rx.recv() => {
                    let mqtt_message = mqtt_message?;
                    let gateway_mac = match &mqtt_message {
                        Message::CellularStatus(mac, _, _)
                        | Message::Punches(mac, _, _)
                        | Message::MeshtasticSerial(mac, _, _)
                        | Message::MeshtasticStatus(mac, _, _) => *mac,
                    };
                    // Instead of ignoring these packets, we could consider unsubscribing from
                    // these topics.
                    if self.local_macs.contains(&gateway_mac) {
                        continue;
                    }
                    if let Some(message) = self.fleet_state.process_mqtt_message(mqtt_message)? {
                        return Ok(message);
                    }
                }
                Some(service_envelope) = self.mesh_packet_rx.recv() => {
                    if let Some(MeshPacket {
                        from,
                        payload_variant: Some(PayloadVariant::Encrypted(_)),
                        ..
                    }) = service_envelope.packet
                    {
                        debug!("Ignoring encrypted message. From node ID={from:x}.");
                        continue;
                    };
                    let maybe_event = self.process_service_envelope(service_envelope).transpose();
                    if let Some(event) = maybe_event {
                        return event;
                    }
                },
                punch_recv = self.si_rx.recv() => {
                    match punch_recv {
                        Some(SportIdentMessage::RawPunch(raw_punch)) => {
                            let now = Local::now().with_timezone(&self.timezone);
                            let punch = SiPunch::from_raw(raw_punch, now.date_naive(), now.offset());
                            return Ok(Event::SiPunch(punch));
                        }
                        Some(SportIdentMessage::DeviceEvent { added, device }) => {
                            return Ok(Event::DeviceEvent { added, device });
                        }
                        None => {} // Can't happen since self has self._si_tx
                    }
                }
                node_infos = self.fleet_state.publish_node_infos() => {
                    return Ok(Event::NodeInfos(node_infos));
                }
                task_res = self.tasks.join_next(), if !self.tasks.is_empty() => {
                    if let Some(Err(e)) = task_res {
                        error!("Background task failed: {e}");
                    }
                }
                else => {
                    error!("Channel closed unexpectedly, this is a bug and it shouldn't happen.");
                }
            }
        }
    }
}

pub struct FakePunchConfig {
    pub interval: Duration,
    pub card: u32,
    pub code: u16,
}

impl Default for FakePunchConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            card: 46283,
            code: 47,
        }
    }
}

/// A builder to construct `MessageHandler` using the builder pattern.
pub struct MessageHandlerBuilder {
    dns: Vec<(String, MacAddress)>,
    mqtt_configs: Vec<MqttConfig>,
    node_infos_interval: Duration,
    meshtastic_timeout: Duration,
    fake_punch_config: Option<FakePunchConfig>,
    config: UsbSerialConfig,
    meshtastic_tcp: Option<String>,
    timezone: FixedOffset,
}

impl Default for MessageHandlerBuilder {
    fn default() -> Self {
        Self {
            dns: Vec::new(),
            mqtt_configs: Vec::new(),
            node_infos_interval: Duration::from_secs(60),
            meshtastic_timeout: Duration::from_secs(600),
            config: UsbSerialConfig::default(),
            meshtastic_tcp: None,
            fake_punch_config: None,
            timezone: *Local::now().fixed_offset().offset(),
        }
    }
}

impl MessageHandlerBuilder {
    /// Creates a new `MessageHandlerBuilder` with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the DNS records for node name resolution.
    pub fn with_dns(mut self, dns: Vec<(String, MacAddress)>) -> Self {
        self.dns = dns;
        self
    }

    /// Sets the MQTT configs to subscribe to.
    pub fn with_mqtt_configs(mut self, configs: Vec<MqttConfig>) -> Self {
        self.mqtt_configs = configs;
        self
    }

    /// Sets the node info publication interval.
    pub fn with_node_infos_interval(mut self, interval: Duration) -> Self {
        self.node_infos_interval = interval;
        self
    }

    /// Sets the Meshtastic node timeout.
    pub fn with_meshtastic_timeout(mut self, timeout: Duration) -> Self {
        self.meshtastic_timeout = timeout;
        self
    }

    /// Sets the USB serial configuration.
    pub fn with_usb_serial_config(mut self, config: UsbSerialConfig) -> Self {
        self.config = config;
        self
    }

    /// Sets the Meshtastic TCP connection host.
    pub fn with_tcp(mut self, host: String) -> Self {
        self.meshtastic_tcp = Some(host);
        self
    }

    /// Sets the interval for sending fake punches (optional).
    pub fn with_fake_punch(
        mut self,
        interval: Duration,
        card: Option<u32>,
        code: Option<u16>,
    ) -> Self {
        let config = FakePunchConfig {
            interval,
            card: card.unwrap_or(46283),
            code: code.unwrap_or(47),
        };
        self.fake_punch_config = Some(config);
        self
    }

    /// Sets the timezone offset to use.
    pub fn with_timezone(mut self, timezone: FixedOffset) -> Self {
        self.timezone = timezone;
        self
    }

    /// Builds the `MessageHandler`.
    pub fn build(self) -> MessageHandler {
        let macs = self.dns.iter().map(|(_, mac)| mac);
        let mqtt_receivers: Vec<MqttReceiver> = self
            .mqtt_configs
            .into_iter()
            .map(|config| MqttReceiver::new(config, macs.clone()))
            .collect();
        let (mesh_packet_tx, mesh_packet_rx) = unbounded_channel::<ServiceEnvelope>();
        let (si_tx, si_rx) = unbounded_channel::<SportIdentMessage>();
        let (mqtt_tx, mqtt_rx) = unbounded_channel::<crate::Result<Message>>();

        let mut factories: Vec<Box<dyn UsbSerialFactory>> = Vec::new();
        if self.config.enable_meshtastic {
            factories.push(Box::new(MeshtasticFactory::new(mesh_packet_tx.clone())));
        }
        match self.config.sportident {
            SportIdentConfig::Passive => {
                factories.push(Box::new(SportIdentFactory::new(si_tx.clone())));
            }
            SportIdentConfig::Active(factory) => {
                factories.push(factory);
            }
            SportIdentConfig::None => {}
        }

        let usb_serial_manager = if factories.is_empty() {
            None
        } else {
            Some(UsbSerialManager::new(factories))
        };

        MessageHandler {
            fleet_state: FleetState::new(
                self.dns,
                self.node_infos_interval,
                self.meshtastic_timeout,
            ),
            mesh_packet_rx,
            _mesh_packet_tx: mesh_packet_tx,
            si_rx,
            si_tx,
            inbound_mqtt_tx: mqtt_tx,
            inbound_mqtt_rx: mqtt_rx,
            mesh_txs: Vec::new(),
            tasks: JoinSet::new(),
            initializer: Some(MessageHandlerInitializer {
                meshtastic_tcp: self.meshtastic_tcp,
                mqtt_receivers,
                fake_punch_config: self.fake_punch_config,
                usb_serial_manager,
            }),
            timezone: self.timezone,
            local_macs: HashSet::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::{DateTime, Local};
    use femtopb::{Message as _, Repeated};
    use tokio::time::timeout;
    use yaroc_common::proto::Punches;
    use yaroc_common::punch::RawPunch;

    type TestChannels = (
        MessageHandler,
        UnboundedSender<SportIdentMessage>,
        UnboundedSender<ServiceEnvelope>,
        UnboundedSender<crate::Result<Message>>,
    );

    impl MessageHandler {
        /// Creates a handler tailored for testing, bypassing factories and exposing senders directly.
        pub fn new_for_test(node_infos_interval: Duration) -> TestChannels {
            let (mesh_packet_tx, mesh_packet_rx) = unbounded_channel();
            let (punch_tx, punch_rx) = unbounded_channel();
            let (mqtt_tx, mqtt_rx) = unbounded_channel();
            let handler = Self {
                fleet_state: FleetState::new(vec![], node_infos_interval, Duration::from_secs(600)),
                mesh_packet_rx,
                _mesh_packet_tx: mesh_packet_tx.clone(),
                si_rx: punch_rx,
                si_tx: punch_tx.clone(),
                inbound_mqtt_tx: mqtt_tx.clone(),
                inbound_mqtt_rx: mqtt_rx,
                mesh_txs: Vec::new(),
                tasks: JoinSet::new(),
                initializer: Some(MessageHandlerInitializer {
                    meshtastic_tcp: None,
                    mqtt_receivers: Vec::new(),
                    fake_punch_config: None,
                    usb_serial_manager: None,
                }),
                timezone: *Local::now().fixed_offset().offset(),
                local_macs: HashSet::new(),
            };
            (handler, punch_tx, mesh_packet_tx, mqtt_tx)
        }
    }

    #[tokio::test]
    async fn test_message_handler_punch_event() {
        let (mut handler, punch_tx, _mesh_tx, _mqtt_tx) =
            MessageHandler::new_for_test(Duration::from_secs(60));

        let mut raw_punch: RawPunch = [0; 20];
        raw_punch[0..3].copy_from_slice(&[1, 2, 3]);
        punch_tx.send(SportIdentMessage::RawPunch(raw_punch)).unwrap();

        let event = timeout(Duration::from_secs(1), handler.next_event())
            .await
            .expect("next_event timed out")
            .expect("next_event failed");

        match event {
            Event::SiPunch(punch) => assert_eq!(punch.raw, raw_punch),
            _ => panic!("Expected Event::SiPunch"),
        }
    }

    #[tokio::test]
    async fn test_message_handler_fake_punch() {
        let mut handler = MessageHandlerBuilder::new()
            .with_fake_punch(Duration::from_millis(10), None, None)
            .build();

        let event = timeout(Duration::from_secs(1), handler.next_event())
            .await
            .expect("next_event timed out")
            .expect("next_event failed");

        match event {
            Event::SiPunch(punch) => {
                assert_eq!(punch.card, 46283);
                assert_eq!(punch.code, 47);
            }
            _ => panic!("Expected Event::SiPunch"),
        }
    }

    #[tokio::test]
    async fn test_message_handler_fake_punch_custom() {
        let mut handler = MessageHandlerBuilder::new()
            .with_fake_punch(Duration::from_millis(10), Some(12345), Some(56))
            .build();

        let event = timeout(Duration::from_secs(1), handler.next_event())
            .await
            .expect("next_event timed out")
            .expect("next_event failed");

        match event {
            Event::SiPunch(punch) => {
                assert_eq!(punch.card, 12345);
                assert_eq!(punch.code, 56);
            }
            _ => panic!("Expected Event::SiPunch"),
        }
    }

    #[tokio::test]
    async fn test_message_handler_device_event() {
        let (mut handler, punch_tx, _mesh_tx, _mqtt_tx) =
            MessageHandler::new_for_test(Duration::from_secs(60));

        punch_tx
            .send(SportIdentMessage::DeviceEvent {
                added: true,
                device: "/dev/ttyUSB0".to_owned(),
            })
            .unwrap();

        let event = timeout(Duration::from_secs(1), handler.next_event())
            .await
            .expect("next_event timed out")
            .expect("next_event failed");

        match event {
            Event::DeviceEvent { added, device } => {
                assert!(added);
                assert_eq!(device, "/dev/ttyUSB0");
            }
            _ => panic!("Expected Event::DeviceEvent"),
        }
    }

    #[tokio::test]
    async fn test_message_handler_node_infos_interval() {
        // Use a short interval to test the timeout branch
        let (mut handler, _punch_tx, _mesh_tx, _mqtt_tx) =
            MessageHandler::new_for_test(Duration::from_millis(50));

        let event = timeout(Duration::from_secs(1), handler.next_event())
            .await
            .expect("next_event timed out")
            .expect("next_event failed");

        match event {
            Event::NodeInfos(_) => {}
            _ => panic!("Expected Event::NodeInfos"),
        }
    }

    #[tokio::test]
    async fn test_message_handler_mqtt_punch() {
        let (mut handler, _punch_tx, _mesh_tx, mqtt_tx) =
            MessageHandler::new_for_test(Duration::from_secs(60));

        let time = DateTime::parse_from_rfc3339("2023-11-23T10:00:03.793+01:00").unwrap();
        let punch = SiPunch::new_send_last_record(1715004, 47, time, 2).raw;
        let punches_slice: &[&[u8]] = &[&punch];
        let punches = Punches {
            punches: Repeated::from_slice(punches_slice),
            ..Default::default()
        };
        let mut buf = vec![0u8; punches.encoded_len()];
        punches.encode(&mut buf.as_mut_slice()).unwrap();

        mqtt_tx
            .send(Ok(crate::mqtt::Message::Punches(
                MacAddress::default(),
                Local::now(),
                buf,
            )))
            .unwrap();

        let event = timeout(Duration::from_secs(1), handler.next_event())
            .await
            .expect("next_event timed out")
            .expect("next_event failed");

        match event {
            Event::SiPunches(punch_logs) => {
                assert_eq!(punch_logs.len(), 1);
                assert_eq!(punch_logs[0].punch.code, 47);
                assert_eq!(punch_logs[0].punch.card, 1715004);
            }
            _ => panic!("Expected Event::SiPunches"),
        }
    }

    #[tokio::test]
    async fn test_message_handler_encrypted_mesh_ignored() {
        let (mut handler, punch_tx, mesh_tx, _mqtt_tx) =
            MessageHandler::new_for_test(Duration::from_secs(60));

        let encrypted_envelope = ServiceEnvelope {
            packet: Some(MeshPacket {
                from: 0xdeadbeef,
                payload_variant: Some(PayloadVariant::Encrypted(vec![1, 2, 3])),
                ..Default::default()
            }),
            gateway_id: "!12345678".to_string(),
            ..Default::default()
        };

        // Send the encrypted envelope. It should be ignored.
        mesh_tx.send(encrypted_envelope).unwrap();

        // Send a device event right after, which should be processed.
        punch_tx
            .send(SportIdentMessage::DeviceEvent {
                added: true,
                device: "/dev/ttyUSB0".to_owned(),
            })
            .unwrap();

        // The next_event call should return the device event, skipping the encrypted packet.
        let event = timeout(Duration::from_secs(1), handler.next_event())
            .await
            .expect("next_event timed out")
            .expect("next_event failed");

        match event {
            Event::DeviceEvent { added, device } => {
                assert!(added);
                assert_eq!(device, "/dev/ttyUSB0");
            }
            _ => panic!("Expected Event::DeviceEvent"),
        }
    }

    #[tokio::test]
    async fn test_message_handler_decrypted_mesh_packet() {
        use meshtastic::protobufs::{Data, PortNum};

        let (mut handler, _punch_tx, mesh_tx, _mqtt_tx) =
            MessageHandler::new_for_test(Duration::from_secs(60));

        let time = DateTime::parse_from_rfc3339("2023-11-23T10:00:03+01:00").unwrap();
        let punch = SiPunch::new_send_last_record(1715004, 47, time, 2).raw;

        let envelope = ServiceEnvelope {
            packet: Some(MeshPacket {
                from: 0xdeadbeef,
                payload_variant: Some(PayloadVariant::Decoded(Data {
                    portnum: PortNum::SerialApp as i32,
                    payload: punch.to_vec(),
                    ..Default::default()
                })),
                ..Default::default()
            }),
            gateway_id: "!12345678".to_string(),
            ..Default::default()
        };

        // Send the decoded (decrypted) envelope. It should be processed successfully.
        mesh_tx.send(envelope).unwrap();

        let event = timeout(Duration::from_secs(1), handler.next_event())
            .await
            .expect("next_event timed out")
            .expect("next_event failed");

        match event {
            Event::SiPunchesMeshtastic(punches, _envelope) => {
                assert_eq!(punches.len(), 1);
                assert_eq!(punches[0].punch.code, 47);
                assert_eq!(punches[0].punch.card, 1715004);
            }
            _ => panic!("Expected Event::SiPunchesMeshtastic"),
        }
    }
}
