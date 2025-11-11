use crate::{
    mqtt::{MqttConfig, MqttReceiver},
    serial_device_manager::SerialDeviceManager,
    state::{Event, FleetState},
    system_info::MacAddress,
};
use futures::future::select_all;
use meshtastic::protobufs::MeshPacket;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Orchestrates the overall message flow.
///
/// This struct is responsible for receiving messages from MQTT and Meshtastic devices,
/// processing them, and maintaining the state of the fleet.
pub struct MessageHandler {
    fleet_state: FleetState,
    mqtt_receivers: Vec<MqttReceiver>,
    mesh_proto_tx: UnboundedSender<(MeshPacket, MacAddress)>,
    mesh_proto_rx: UnboundedReceiver<(MeshPacket, MacAddress)>,
}

impl MessageHandler {
    /// Creates a new `MessageHandler`.
    ///
    /// This function initializes the `FleetState` and an optional `MqttReceiver`.
    pub fn new(dns: Vec<(String, MacAddress)>, mqtt_configs: Vec<MqttConfig>) -> Self {
        let macs = dns.iter().map(|(_, mac)| mac);
        let mqtt_receivers = mqtt_configs
            .into_iter()
            .map(|config| MqttReceiver::new(config, macs.clone()))
            .collect();
        let (mesh_proto_tx, mesh_proto_rx) = unbounded_channel::<(MeshPacket, MacAddress)>();
        Self {
            fleet_state: FleetState::new(dns, Duration::from_secs(60)),
            mqtt_receivers,
            mesh_proto_tx,
            mesh_proto_rx,
        }
    }

    /// Returns the next event.
    ///
    /// This function is a long-running task that should be polled.
    pub async fn next_event(&mut self) -> crate::Result<Event> {
        loop {
            let mqtt_futures: Vec<_> = self
                .mqtt_receivers
                .iter_mut()
                .map(|receiver: &mut MqttReceiver| Box::pin(receiver.next_message()))
                .collect();
            tokio::select! {
                (mqtt_message, _idx, _) = select_all(mqtt_futures.into_iter()) => {
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

    /// Returns a new `SerialDeviceManager` that can be used to handle Meshtastic devices.
    pub fn meshtastic_device_handler(&self) -> SerialDeviceManager {
        SerialDeviceManager::new(self.mesh_proto_tx.clone())
    }
}
