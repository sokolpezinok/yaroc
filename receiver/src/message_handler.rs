use crate::meshtastic_serial::MeshtasticFactory;
use crate::meshtastic_tcp::connect_and_loop;
use crate::si_uart::{SportIdentFactory, SportIdentMessage};
use crate::usb_serial_manager::{UsbSerialFactory, UsbSerialManager};
use crate::{
    mqtt::{Message, MqttConfig, MqttReceiver},
    state::{Event, FleetState},
    system_info::MacAddress,
};
use chrono::Local;
use log::{error, info};
use meshtastic::protobufs::MeshPacket;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinSet;
use yaroc_common::punch::SiPunch;

pub struct MessageHandlerInitializer {
    pub meshtastic_tcp: Option<String>,
    pub mqtt_receivers: Vec<MqttReceiver>,
    pub fake_punch_interval: Option<Duration>,
    pub usb_serial_manager: Option<UsbSerialManager>,
}

/// Orchestrates the overall message flow.
///
/// This struct is responsible for receiving messages from MQTT, SportIdent and Meshtastic devices,
/// processing them, and maintaining the state of the fleet.
pub struct MessageHandler {
    fleet_state: FleetState,
    mesh_packet_rx: UnboundedReceiver<(MeshPacket, MacAddress)>,
    _mesh_packet_tx: UnboundedSender<(MeshPacket, MacAddress)>, // Kept to prevent channel from closing
    si_rx: UnboundedReceiver<SportIdentMessage>,
    si_tx: UnboundedSender<SportIdentMessage>, // Kept to prevent channel from closing
    mqtt_tx: UnboundedSender<crate::Result<Message>>,
    mqtt_rx: UnboundedReceiver<crate::Result<Message>>,
    tasks: JoinSet<()>,
    initializer: Option<MessageHandlerInitializer>,
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
    pub async fn init(&mut self) {
        if let Some(init) = self.initializer.take() {
            // Initialize Meshtastic TCP connection if configured and when run for the first time.
            if let Some(host) = init.meshtastic_tcp {
                let mesh_packet_tx = self._mesh_packet_tx.clone();
                self.tasks.spawn(async move {
                    connect_and_loop(host, mesh_packet_tx).await;
                });
            }

            if let Some(interval) = init.fake_punch_interval {
                let si_tx = self.si_tx.clone();
                info!("Starting a fake SportIdent worker, sending a punch every {interval:?}");
                self.tasks.spawn(async move {
                    let mut interval_timer = tokio::time::interval(interval);
                    // The first tick of tokio::time::interval completes immediately.
                    loop {
                        interval_timer.tick().await;
                        let now = Local::now().fixed_offset();
                        let punch = SiPunch::new_send_last_record(46283, 47, now, 18);
                        if let Err(e) = si_tx.send(SportIdentMessage::RawPunch(punch.raw)) {
                            error!("Failed to send fake punch: {e}");
                            break;
                        }
                    }
                });
            }

            // Spawn MQTT tasks when `next_event()` is run for the first time.
            for mut receiver in init.mqtt_receivers {
                let mqtt_tx = self.mqtt_tx.clone();
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
                mqtt_msg = self.mqtt_rx.recv() => {
                    // None can't happen since self holds a copy of mqtt_tx
                    if let Some(mqtt_message) = mqtt_msg
                        && let Some(message) = self.fleet_state.process_message(mqtt_message?)?
                    {
                        return Ok(message);
                    }
                }
                mesh_recv = self.mesh_packet_rx.recv() => {
                    // None can't happen since self holds a copy of _mesh_packet_tx
                    if let Some((mesh_packet, mac_address)) = mesh_recv
                        && let Some(message) = self.fleet_state.process_mesh_packet(mesh_packet, mac_address)?
                    {
                        return Ok(message);
                    }
                },
                punch_recv = self.si_rx.recv() => {
                    match punch_recv {
                        Some(SportIdentMessage::RawPunch(raw_punch)) => {
                            let now = Local::now().fixed_offset();
                            let punch = SiPunch::from_raw(raw_punch, now.date_naive(), now.offset());
                            return Ok(Event::SiPunch(punch));
                        }
                        Some(SportIdentMessage::DeviceEvent { added, device }) => {
                            return Ok(Event::DeviceEvent { added, device });
                        }
                        None => {} // Can't happen since self holds a copy of _si_tx
                    }
                }
                node_infos = self.fleet_state.publish_node_infos() => {
                    return Ok(Event::NodeInfos(node_infos));
                }
                Some(task_res) = self.tasks.join_next(), if !self.tasks.is_empty() => {
                    if let Err(e) = task_res {
                        error!("Background task failed: {e}");
                    }
                }
            }
        }
    }
}

/// A builder to construct `MessageHandler` using the builder pattern.
pub struct MessageHandlerBuilder {
    dns: Vec<(String, MacAddress)>,
    mqtt_configs: Vec<MqttConfig>,
    node_infos_interval: Duration,
    meshtastic_timeout: Duration,
    config: UsbSerialConfig,
    meshtastic_tcp: Option<String>,
    fake_punch_interval: Option<Duration>,
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
            fake_punch_interval: None,
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
    pub fn with_fake_punch(mut self, interval: Duration) -> Self {
        self.fake_punch_interval = Some(interval);
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
        let (mesh_packet_tx, mesh_packet_rx) = unbounded_channel::<(MeshPacket, MacAddress)>();
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
            mqtt_tx,
            mqtt_rx,
            tasks: JoinSet::new(),
            initializer: Some(MessageHandlerInitializer {
                meshtastic_tcp: self.meshtastic_tcp,
                mqtt_receivers,
                fake_punch_interval: self.fake_punch_interval,
                usb_serial_manager,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::time::timeout;
    use yaroc_common::punch::RawPunch;

    type TestChannels = (
        MessageHandler,
        UnboundedSender<SportIdentMessage>,
        UnboundedSender<(MeshPacket, MacAddress)>,
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
                mqtt_tx: mqtt_tx.clone(),
                mqtt_rx,
                tasks: JoinSet::new(),
                initializer: Some(MessageHandlerInitializer {
                    meshtastic_tcp: None,
                    mqtt_receivers: Vec::new(),
                    fake_punch_interval: None,
                    usb_serial_manager: None,
                }),
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
        let mut handler =
            MessageHandlerBuilder::new().with_fake_punch(Duration::from_millis(10)).build();

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
        use chrono::{DateTime, Local};
        use femtopb::{Message as _, Repeated};
        use yaroc_common::proto::Punches;
        use yaroc_common::punch::SiPunch;

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
}
