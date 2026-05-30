use crate::meshtastic_serial::MeshtasticFactory;
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
        config: UsbSerialConfig,
    ) -> (Self, UsbSerialManager) {
        let macs = dns.iter().map(|(_, mac)| mac);
        let mqtt_receivers: Vec<MqttReceiver> = mqtt_configs
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
        if config.enable_meshtastic {
            factories.push(Box::new(MeshtasticFactory::new(mesh_packet_tx)));
        }
        match config.sportident {
            SportIdentConfig::Passive => {
                factories.push(Box::new(SportIdentFactory::new(punch_tx)));
            }
            SportIdentConfig::Active(factory) => {
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
