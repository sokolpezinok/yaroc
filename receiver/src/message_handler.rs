use crate::{
    mqtt::{Message, MqttConfig, MqttReceiver},
    state::{Event, FleetState},
    system_info::MacAddress,
    usb_serial_manager::UsbSerialManager,
};
use futures::future::select_all;
use meshtastic::protobufs::MeshPacket;
use std::{future::pending, pin::Pin, time::Duration};
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
    pub fn new(
        dns: Vec<(String, MacAddress)>,
        mqtt_configs: Vec<MqttConfig>,
        node_infos_interval: Duration,
    ) -> Self {
        let macs = dns.iter().map(|(_, mac)| mac);
        let mqtt_receivers = mqtt_configs
            .into_iter()
            .map(|config| MqttReceiver::new(config, macs.clone()))
            .collect();
        let (mesh_proto_tx, mesh_proto_rx) = unbounded_channel::<(MeshPacket, MacAddress)>();
        Self {
            fleet_state: FleetState::new(dns, node_infos_interval),
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
            let mut mqtt_futures: Vec<_> = self
                .mqtt_receivers
                .iter_mut()
                .map(|receiver: &mut MqttReceiver| {
                    Box::pin(receiver.next_message())
                        as Pin<Box<dyn Future<Output = crate::Result<Message>> + Send>>
                })
                .collect();
            if mqtt_futures.is_empty() {
                mqtt_futures.push(Box::pin(pending::<crate::Result<Message>>()));
            }
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

    /// Returns a new `UsbSerialManager` that can be used to handle Meshtastic and SportIdent devices.
    pub fn usb_serial_manager(&self, enable_meshtastic: bool) -> UsbSerialManager {
        let mesh_tx = if enable_meshtastic {
            Some(self.mesh_proto_tx.clone())
        } else {
            None
        };
        UsbSerialManager::new(mesh_tx, None)
    }
}
