use std::time::Duration;

use meshtastic::protobufs::MeshPacket;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::meshtastic_serial::MshDevHandler;
use crate::mqtt::{MqttConfig, MqttReceiver};
use crate::state::{Event, FleetState};
use crate::system_info::MacAddress;

/// Orchestrates the overall message flow.
///
/// This struct is responsible for receiving messages from MQTT and Meshtastic devices,
/// processing them, and maintaining the state of the fleet.
pub struct MessageHandler {
    fleet_state: FleetState,
    mqtt_receiver: Option<MqttReceiver>,
    mesh_proto_tx: UnboundedSender<(MeshPacket, MacAddress)>,
    mesh_proto_rx: UnboundedReceiver<(MeshPacket, MacAddress)>,
}

impl MessageHandler {
    /// Creates a new `MessageHandler`.
    ///
    /// This function initializes the `FleetState` and an optional `MqttReceiver`.
    pub fn new(dns: Vec<(String, MacAddress)>, mqtt_config: Option<MqttConfig>) -> Self {
        let macs = dns.iter().map(|(_, mac)| mac);
        //TODO: allow multiple MQTT receivers
        let mqtt_receiver = mqtt_config.map(|config| MqttReceiver::new(config, macs));
        let (mesh_proto_tx, mesh_proto_rx) = unbounded_channel::<(MeshPacket, MacAddress)>();
        Self {
            fleet_state: FleetState::new(dns, Duration::from_secs(60)),
            mqtt_receiver,
            mesh_proto_tx,
            mesh_proto_rx,
        }
    }

    /// Returns the next event.
    ///
    /// This function is a long-running task that should be polled.
    pub async fn next_event(&mut self) -> crate::Result<Event> {
        loop {
            tokio::select! {
                mqtt_message = async {
                    match self.mqtt_receiver.as_mut() {
                        Some(receiver) => receiver.next_message().await,
                        None => std::future::pending().await
                    }
                } => {
                    if let Some(message) = self.fleet_state.process_message(mqtt_message?)? {
                        return Ok(message);
                    }
                }
                mesh_recv = self.mesh_proto_rx.recv() => {
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
                node_infos = self.fleet_state.publish_node_infos() => {
                    return Ok(Event::NodeInfos(node_infos));
                }
            }
        }
    }

    /// Returns a new `MshDevHandler` that can be used to handle Meshtastic devices.
    pub fn meshtastic_device_handler(&self) -> MshDevHandler {
        MshDevHandler::new(self.mesh_proto_tx.clone())
    }
}
