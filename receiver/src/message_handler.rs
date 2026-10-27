use chrono::{FixedOffset, Local};
use futures::{
    StreamExt,
    stream::{BoxStream, SelectAll},
};
use log::error;
use meshtastic::protobufs::ServiceEnvelope;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::DropGuard;

use crate::meshtastic::tcp;
pub use crate::si_uart::FakePunchConfig;
use crate::usb_serial_manager::UsbSerialManager;
pub use crate::usb_serial_manager::{SportIdentConfig, UsbSerialConfig};
use crate::{
    mqtt::{Message, MqttConfig, MqttReceiver},
    state::{Event, FleetState},
    system_info::MacAddress,
};

/// Orchestrates the overall message flow.
///
/// This struct is responsible for receiving messages from MQTT, SportIdent and Meshtastic devices,
/// processing them, and maintaining the state of the fleet.
pub struct MessageHandler {
    fleet_state: FleetState,
    mesh_streams: SelectAll<BoxStream<'static, ServiceEnvelope>>,
    si_streams: SelectAll<BoxStream<'static, Event>>,
    /// Multiplexed streams of inbound MQTT messages
    mqtt_streams: SelectAll<BoxStream<'static, crate::Result<Message>>>,
    /// Senders for forwarding local Meshtastic messages to MQTT
    mesh_to_mqtt_txs: Vec<UnboundedSender<ServiceEnvelope>>,
    usb_serial_manager: Option<UsbSerialManager>,
    _usb_drop_guard: Option<DropGuard>,
    timezone: FixedOffset,
    local_macs: HashSet<MacAddress>,
}

impl MessageHandler {
    /// Spawn async background tasks when Self::next_event() is ran for the first time.
    pub fn init(&mut self) {
        if let Some(usb_manager) = self.usb_serial_manager.take() {
            self._usb_drop_guard = Some(usb_manager.spawn());
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
        for mesh_to_mqtt_tx in &self.mesh_to_mqtt_txs {
            let _ = mesh_to_mqtt_tx
                .send(service_envelope.clone())
                .inspect_err(|e| error!("Error while forwarding meshtastic packets: {}", e));
        }
        self.fleet_state.process_mesh_packet(service_envelope, mac_address, now)
    }

    /// Returns the next processed event from the active event sources.
    ///
    /// This function asynchronously polls multiple message streams (MQTT background tasks, Meshtastic
    /// packet receiver stream, SportIdent event stream, and node info publication interval)
    /// using `tokio::select!`. It returns the first successfully decoded and processed `Event`.
    ///
    /// # Errors
    ///
    /// Returns an error if any of the underlying event parsing or state updates fail.
    pub async fn next_event(&mut self) -> crate::Result<Event> {
        self.init();
        loop {
            tokio::select! {
                Some(mqtt_message) = self.mqtt_streams.next(), if !self.mqtt_streams.is_empty() => {
                    let mqtt_message = mqtt_message?;
                    // Instead of ignoring these packets, we could consider unsubscribing from
                    // these topics.
                    if self.local_macs.contains(&mqtt_message.mac_address()) {
                        continue;
                    }
                    if let Some(message) = self.fleet_state.process_mqtt_message(mqtt_message)? {
                        return Ok(message);
                    }
                }
                Some(service_envelope) = self.mesh_streams.next(), if !self.mesh_streams.is_empty() => {
                    if let Some(event) = self.process_service_envelope(service_envelope)? {
                        return Ok(event);
                    }
                }
                Some(event) = self.si_streams.next(), if !self.si_streams.is_empty() => {
                    return Ok(event);
                }
                node_infos = self.fleet_state.publish_node_infos() => {
                    return Ok(Event::NodeInfos(node_infos));
                }
                else => {
                    error!("Channel closed unexpectedly, this is a bug and it shouldn't happen.");
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
        let mut mqtt_streams = SelectAll::new();
        let mut mesh_to_mqtt_txs = Vec::with_capacity(self.mqtt_configs.len());
        for config in self.mqtt_configs {
            let receiver = MqttReceiver::new(config, macs.clone());
            mesh_to_mqtt_txs.push(receiver.mesh_tx());
            mqtt_streams.push(receiver.into_stream());
        }

        let (usb_serial_manager, usb_mesh_stream, usb_si_stream) =
            UsbSerialManager::from_config(self.config, self.timezone);

        let mut mesh_streams = SelectAll::new();
        if let Some(host) = self.meshtastic_tcp {
            mesh_streams.push(tcp::into_stream(host).boxed());
        }
        if let Some(stream) = usb_mesh_stream {
            mesh_streams.push(stream.boxed());
        }

        let mut si_streams = SelectAll::new();
        if let Some(fake_punch_config) = self.fake_punch_config {
            si_streams.push(fake_punch_config.into_stream(self.timezone).boxed());
        }
        if let Some(stream) = usb_si_stream {
            si_streams.push(stream.boxed());
        }

        MessageHandler {
            fleet_state: FleetState::new(
                self.dns,
                self.node_infos_interval,
                self.meshtastic_timeout,
            ),
            mesh_streams,
            si_streams,
            mqtt_streams,
            mesh_to_mqtt_txs,
            usb_serial_manager,
            _usb_drop_guard: None,
            timezone: self.timezone,
            local_macs: HashSet::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::si_uart::SportIdentMessage;
    use chrono::{DateTime, Local};
    use femtopb::{Message as _, Repeated};
    use futures::Stream;
    use meshtastic::protobufs::PortNum;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
    use tokio::time::timeout;
    use yaroc_common::proto::Punches;
    use yaroc_common::punch::{RawPunch, SiPunch};

    type TestChannels = (
        MessageHandler,
        UnboundedSender<SportIdentMessage>,
        UnboundedSender<ServiceEnvelope>,
        UnboundedSender<crate::Result<Message>>,
    );

    impl MessageHandler {
        /// Converts an `UnboundedReceiver` into a `Stream`.
        fn rx_into_stream<T>(rx: UnboundedReceiver<T>) -> impl Stream<Item = T> {
            futures::stream::unfold(
                rx,
                |mut rx| async move { rx.recv().await.map(|msg| (msg, rx)) },
            )
        }

        /// Creates a handler tailored for testing, bypassing factories and exposing senders directly.
        pub fn new_for_test(node_infos_interval: Duration) -> TestChannels {
            let (mesh_packet_tx, mesh_packet_rx) = unbounded_channel();
            let (punch_tx, punch_rx) = unbounded_channel();
            let (mqtt_tx, mqtt_rx) = unbounded_channel();
            let mut mqtt_streams = SelectAll::new();
            mqtt_streams.push(Self::rx_into_stream(mqtt_rx).boxed());
            let timezone = *Local::now().fixed_offset().offset();
            let mesh_streams = SelectAll::from_iter([Self::rx_into_stream(mesh_packet_rx).boxed()]);
            let si_streams = SelectAll::from_iter([Self::rx_into_stream(punch_rx)
                .map(move |msg: SportIdentMessage| msg.into_event(timezone))
                .boxed()]);
            let handler = Self {
                fleet_state: FleetState::new(vec![], node_infos_interval, Duration::from_secs(600)),
                mesh_streams,
                si_streams,
                mqtt_streams,
                mesh_to_mqtt_txs: Vec::new(),
                usb_serial_manager: None,
                _usb_drop_guard: None,
                timezone,
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

    #[tokio::test(start_paused = true)]
    async fn test_message_handler_node_infos_interval() {
        let (mut handler, _punch_tx, _mesh_tx, _mqtt_tx) =
            MessageHandler::new_for_test(Duration::from_secs(60));

        let task = tokio::spawn(async move { handler.next_event().await });
        tokio::time::advance(Duration::from_secs(60)).await;

        let event = task.await.unwrap().expect("next_event failed");
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

        let encrypted_envelope =
            crate::test_utils::encrypted_service_envelope(0xdeadbeef, vec![1, 2, 3], "!12345678");

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
        let (mut handler, _punch_tx, mesh_tx, _mqtt_tx) =
            MessageHandler::new_for_test(Duration::from_secs(60));

        let time = DateTime::parse_from_rfc3339("2023-11-23T10:00:03+01:00").unwrap();
        let punch = SiPunch::new_send_last_record(1715004, 47, time, 2).raw;

        let envelope = crate::test_utils::decoded_service_envelope(
            0xdeadbeef,
            PortNum::SerialApp,
            punch.to_vec(),
            "!12345678",
        );

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
