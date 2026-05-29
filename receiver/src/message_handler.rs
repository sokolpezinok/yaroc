use crate::meshtastic::MeshtasticFactory;
use crate::si_uart::{SportIdentFactory, SportIdentMessage};
use crate::usb_serial_manager::{UsbSerialFactory, UsbSerialManager};
use crate::{
    mqtt::{Message, MqttConfig, MqttReceiver},
    state::{Event, FleetState},
    system_info::MacAddress,
};
use chrono::Local;
use futures::future::select_all;
use meshtastic::protobufs::MeshPacket;
use std::{future::pending, pin::Pin, time::Duration};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use yaroc_common::punch::SiPunch;

/// Orchestrates the overall message flow.
///
/// This struct is responsible for receiving messages from MQTT and Meshtastic devices,
/// processing them, and maintaining the state of the fleet.
pub struct MessageHandler {
    fleet_state: FleetState,
    mqtt_receivers: Vec<MqttReceiver>,
    mesh_packet_rx: UnboundedReceiver<(MeshPacket, MacAddress)>,
    punch_rx: UnboundedReceiver<SportIdentMessage>,
}

impl MessageHandler {
    /// Creates a new `MessageHandler` along with a `UsbSerialManager`.
    ///
    /// This function initializes the `FleetState`, the optional `MqttReceiver`s,
    /// and the `UsbSerialManager`.
    pub fn new(
        dns: Vec<(String, MacAddress)>,
        mqtt_configs: Vec<MqttConfig>,
        node_infos_interval: Duration,
        enable_meshtastic: bool,
        enable_sportident: bool,
    ) -> (Self, UsbSerialManager) {
        let macs = dns.iter().map(|(_, mac)| mac);
        let mqtt_receivers = mqtt_configs
            .into_iter()
            .map(|config| MqttReceiver::new(config, macs.clone()))
            .collect();
        let (mesh_packet_tx, mesh_packet_rx) = unbounded_channel::<(MeshPacket, MacAddress)>();
        let (punch_tx, punch_rx) = unbounded_channel::<SportIdentMessage>();
        let handler = Self {
            fleet_state: FleetState::new(dns, node_infos_interval),
            mqtt_receivers,
            mesh_packet_rx,
            punch_rx,
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
