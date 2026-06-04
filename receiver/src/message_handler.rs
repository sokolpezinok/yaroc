use crate::meshtastic_serial::MeshtasticFactory;
use crate::si_uart::{SportIdentFactory, SportIdentMessage};
use crate::usb_serial_manager::{UsbSerialFactory, UsbSerialManager};
use crate::{
    mqtt::{Message, MqttConfig, MqttReceiver},
    state::{Event, FleetState},
    system_info::MacAddress,
};
use chrono::Local;
use log::error;
use meshtastic::protobufs::MeshPacket;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinSet;
use yaroc_common::punch::SiPunch;

/// Orchestrates the overall message flow.
///
/// This struct is responsible for receiving messages from MQTT and Meshtastic devices,
/// processing them, and maintaining the state of the fleet.
pub struct MessageHandler {
    fleet_state: FleetState,
    mesh_packet_rx: UnboundedReceiver<(MeshPacket, MacAddress)>,
    punch_rx: UnboundedReceiver<SportIdentMessage>,
    mqtt_receivers: Option<Vec<MqttReceiver>>,
    mqtt_tx: UnboundedSender<crate::Result<Message>>,
    mqtt_rx: UnboundedReceiver<crate::Result<Message>>,
    tasks: JoinSet<()>,
}

pub enum SportIdentConfig {
    None,
    Passive,
    Active(Box<dyn UsbSerialFactory>),
}

pub struct UsbSerialConfig {
    pub enable_meshtastic: bool,
    pub sportident: SportIdentConfig,
}

impl MessageHandler {
    /// Creates a new `MessageHandler` along with a `UsbSerialManager`.
    ///
    /// This function initializes the `FleetState`, sets up background tasks for the optional
    /// `MqttReceiver`s, and prepares the `UsbSerialManager` with appropriate factory instances.
    ///
    /// # Arguments
    ///
    /// * `dns` - A mapping of DNS-like host names to device MAC addresses.
    /// * `mqtt_configs` - Configs for the MQTT servers to connect to and listen for messages.
    /// * `node_infos_interval` - The interval at which node info updates are published.
    /// * `meshtastic_timeout` - Timeout for Meshtastic nodes.
    /// * `config` - Configuration for connected serial devices.
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// 1. The initialized `MessageHandler`.
    /// 2. The `UsbSerialManager` that oversees the USB serial devices.
    pub fn new(
        dns: Vec<(String, MacAddress)>,
        mqtt_configs: Vec<MqttConfig>,
        node_infos_interval: Duration,
        meshtastic_timeout: Duration,
        config: UsbSerialConfig,
    ) -> (Self, UsbSerialManager) {
        let macs = dns.iter().map(|(_, mac)| mac);
        let mqtt_receivers: Vec<MqttReceiver> = mqtt_configs
            .into_iter()
            .map(|config| MqttReceiver::new(config, macs.clone()))
            .collect();
        let (mesh_packet_tx, mesh_packet_rx) = unbounded_channel::<(MeshPacket, MacAddress)>();
        let (punch_tx, punch_rx) = unbounded_channel::<SportIdentMessage>();
        let (mqtt_tx, mqtt_rx) = unbounded_channel::<crate::Result<Message>>();

        let handler = Self {
            fleet_state: FleetState::new(dns, node_infos_interval, meshtastic_timeout),
            mesh_packet_rx,
            punch_rx,
            mqtt_receivers: Some(mqtt_receivers),
            mqtt_tx,
            mqtt_rx,
            tasks: JoinSet::new(),
        };

        let mut factories: Vec<Box<dyn UsbSerialFactory>> = Vec::new();
        if config.enable_meshtastic {
            factories.push(Box::new(MeshtasticFactory::new(mesh_packet_tx)));
        }
        match config.sportident {
            SportIdentConfig::Passive => {
                factories.push(Box::new(SportIdentFactory::new(punch_tx)));
            }
            SportIdentConfig::Active(factory) => {
                // Note: The `punch_tx` channel is intentionally not passed to the active factory here.
                // The Active variant is used by the Python SerialClient, which handles the punches
                // natively on the Python side, so they don't need to be routed through this MessageHandler.
                factories.push(factory);
            }
            SportIdentConfig::None => {}
        }
        (handler, UsbSerialManager::new(factories))
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
        // Spawn MQTT tasks when next_event is run for the first time.
        if let Some(receivers) = self.mqtt_receivers.take() {
            for mut receiver in receivers {
                let mqtt_tx = self.mqtt_tx.clone();
                self.tasks.spawn(async move {
                    loop {
                        let msg = receiver.next_message().await;
                        let res =
                            mqtt_tx.send(msg).map_err(|_| crate::error::Error::ChannelSendError);
                        if let Err(e) = res {
                            // TODO: print also which receiver/server it is
                            error!("Error while receiving and forwarding MQTT message: {e}");
                        }
                    }
                });
            }
        }

        loop {
            tokio::select! {
                mqtt_msg = self.mqtt_rx.recv() => {
                    match mqtt_msg {
                        Some(mqtt_message) => {
                            if let Some(message) = self.fleet_state.process_message(mqtt_message?)? {
                                return Ok(message);
                            }
                        }
                        None => {
                            //TODO: closed channel
                        }
                    }
                }
                mesh_recv = self.mesh_packet_rx.recv() => {
                    match mesh_recv {
                        Some((mesh_packet, mac_address)) => {
                            if let Some(message) = self.fleet_state.process_mesh_packet(mesh_packet, mac_address)? {
                                return Ok(message);
                            }
                        }
                        None => {
                            //TODO: closed channel
                        }
                    }
                },
                punch_recv = self.punch_rx.recv() => {
                    match punch_recv {
                        Some(SportIdentMessage::RawPunch(raw_punch)) => {
                            let now = Local::now().fixed_offset();
                            let punch = SiPunch::from_raw(raw_punch, now.date_naive(), now.offset());
                            return Ok(Event::SiPunch(punch));
                        }
                        Some(SportIdentMessage::DeviceEvent { added, device }) => {
                            return Ok(Event::DeviceEvent { added, device });
                        }
                        None => {
                            //TODO: closed channel
                        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::time::timeout;
    use yaroc_common::punch::RawPunch;

    impl MessageHandler {
        /// Creates a handler tailored for testing, bypassing factories and exposing senders directly.
        pub fn new_for_test(
            node_infos_interval: Duration,
        ) -> (
            Self,
            UnboundedSender<SportIdentMessage>,
            UnboundedSender<(MeshPacket, MacAddress)>,
            UnboundedSender<crate::Result<Message>>,
        ) {
            let (mesh_packet_tx, mesh_packet_rx) = unbounded_channel();
            let (punch_tx, punch_rx) = unbounded_channel();
            let (mqtt_tx, mqtt_rx) = unbounded_channel();
            let handler = Self {
                fleet_state: FleetState::new(vec![], node_infos_interval, Duration::from_secs(600)),
                mesh_packet_rx,
                punch_rx,
                mqtt_receivers: None,
                mqtt_tx: mqtt_tx.clone(),
                mqtt_rx,
                tasks: JoinSet::new(),
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
