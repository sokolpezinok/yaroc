use crate::usb_serial_manager::{SportIdentMessage, UsbSerialManager};
use crate::{
    mqtt::{Message, MqttConfig, MqttReceiver},
    state::{Event, FleetState},
    system_info::MacAddress,
};
use chrono::Local;
use futures::future::select_all;
use meshtastic::protobufs::MeshPacket;
use std::{future::pending, pin::Pin, time::Duration};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use yaroc_common::punch::SiPunch;

/// Orchestrates the overall message flow.
///
/// This struct is responsible for receiving messages from MQTT and Meshtastic devices,
/// processing them, and maintaining the state of the fleet.
pub struct MessageHandler {
    fleet_state: FleetState,
    mqtt_receivers: Vec<MqttReceiver>,
    mesh_proto_tx: UnboundedSender<(MeshPacket, MacAddress)>,
    mesh_proto_rx: UnboundedReceiver<(MeshPacket, MacAddress)>,
    punch_tx: UnboundedSender<SportIdentMessage>,
    punch_rx: UnboundedReceiver<SportIdentMessage>,
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
        let (punch_tx, punch_rx) = unbounded_channel::<SportIdentMessage>();
        Self {
            fleet_state: FleetState::new(dns, node_infos_interval),
            mqtt_receivers,
            mesh_proto_tx,
            mesh_proto_rx,
            punch_tx,
            punch_rx,
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
                // TODO: solve this differently than tokio::select! on select_all
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

    /// Returns a new `UsbSerialManager` that can be used to handle Meshtastic and SportIdent devices.
    pub fn usb_serial_manager(
        &self,
        enable_meshtastic: bool,
        enable_sportident: bool,
    ) -> UsbSerialManager {
        let mesh_tx = if enable_meshtastic {
            Some(self.mesh_proto_tx.clone())
        } else {
            None
        };
        let si_tx = if enable_sportident {
            Some(self.punch_tx.clone())
        } else {
            None
        };
        UsbSerialManager::new(mesh_tx, si_tx)
    }
}
