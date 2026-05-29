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
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use yaroc_common::punch::SiPunch;

/// Orchestrates the overall message flow.
///
/// This struct is responsible for receiving messages from MQTT and Meshtastic devices,
/// processing them, and maintaining the state of the fleet.
pub struct MessageHandler {
    fleet_state: FleetState,
    mqtt_rx: UnboundedReceiver<crate::Result<Message>>,
    mesh_packet_rx: UnboundedReceiver<(MeshPacket, MacAddress)>,
    punch_rx: UnboundedReceiver<SportIdentMessage>,
    mqtt_tasks: Vec<tokio::task::JoinHandle<()>>,
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
    /// * `enable_meshtastic` - If true, initializes a `MeshtasticFactory` to receive serial data.
    /// * `enable_sportident` - If true, initializes a `SportIdentFactory` to receive raw punches.
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
        enable_meshtastic: bool,
        enable_sportident: bool,
    ) -> (Self, UsbSerialManager) {
        let macs = dns.iter().map(|(_, mac)| mac);
        let mqtt_receivers: Vec<MqttReceiver> = mqtt_configs
            .into_iter()
            .map(|config| MqttReceiver::new(config, macs.clone()))
            .collect();
        let (mesh_packet_tx, mesh_packet_rx) = unbounded_channel::<(MeshPacket, MacAddress)>();
        let (punch_tx, punch_rx) = unbounded_channel::<SportIdentMessage>();

        let (mqtt_tx, mqtt_rx) = unbounded_channel::<crate::Result<Message>>();
        let mut mqtt_tasks = Vec::new();
        for mut receiver in mqtt_receivers {
            let mqtt_tx = mqtt_tx.clone();
            let task = tokio::spawn(async move {
                loop {
                    let msg = receiver.next_message().await;
                    let res = msg.and_then(|msg| {
                        mqtt_tx.send(Ok(msg)).map_err(|_| crate::error::Error::ChannelSendError)
                    });
                    if let Err(e) = res {
                        // TODO: print also which receiver/server it is
                        error!("Error while receiving and forwarding MQTT message: {e}");
                    }
                }
            });
            mqtt_tasks.push(task);
        }

        let handler = Self {
            fleet_state: FleetState::new(dns, node_infos_interval),
            mqtt_rx,
            mesh_packet_rx,
            punch_rx,
            mqtt_tasks,
        };

        let mut factories: Vec<Box<dyn UsbSerialFactory>> = Vec::new();
        if enable_meshtastic {
            factories.push(Box::new(MeshtasticFactory::new(mesh_packet_tx)));
        }
        if enable_sportident {
            factories.push(Box::new(SportIdentFactory::new(punch_tx)));
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
        loop {
            tokio::select! {
                mqtt_recv = self.mqtt_rx.recv() => {
                    match mqtt_recv {
                        Some(mqtt_message) => {
                            if let Some(message) = self.fleet_state.process_message(mqtt_message?)? {
                                return Ok(message);
                            }
                        }
                        None => {
                            // All MQTT receivers closed their channels (exited)
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
            }
        }
    }
}

impl Drop for MessageHandler {
    /// Cleans up the `MessageHandler` by aborting all spawned MQTT receiver background tasks.
    fn drop(&mut self) {
        for task in &self.mqtt_tasks {
            task.abort();
        }
    }
}
