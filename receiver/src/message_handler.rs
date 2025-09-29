use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::time::Duration;

use log::{error, info, warn};
use meshtastic::protobufs::MeshPacket;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;

use crate::meshtastic_serial::{MeshProto, MeshtasticSerial};
use crate::mqtt::{MqttConfig, MqttReceiver};
use crate::state::{Event, FleetState};
use crate::system_info::MacAddress;

/// Handles connecting and disconnecting of Meshtastic devices.
///
/// Currently, only serial port connections are supported.
pub struct MshDevHandler {
    cancellation_tokens: HashMap<String, CancellationToken>,
    mesh_proto_tx: UnboundedSender<(MeshPacket, MacAddress)>,
}

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

/// Meshtastic device handler
///
/// Handles connecting and disconnecting of meshtastic devices. Supports only serial port
/// connections right now.
impl MshDevHandler {
    /// Connects to a Meshtastic device at a given serial port and device node.
    ///
    /// This function spawns a task to handle messages from the device.
    pub async fn add_device(&mut self, port: String, device_node: String) {
        //TODO: make timeout configurable
        match MeshtasticSerial::new(&port, &device_node, Duration::from_secs(12)).await {
            Ok(msh_serial) => {
                let mac_address = msh_serial.mac_address();
                info!("Connected to meshtastic device: {mac_address} at {port}");
                let token = self.spawn_serial(msh_serial);
                self.cancellation_tokens.insert(device_node.to_owned(), token);
            }
            Err(err) => {
                //TODO: return the error
                error!("Error connecting to {port}: {err}");
            }
        }
    }

    /// Disconnects a Meshtastic device.
    ///
    /// This function cancels the task that handles messages from the device.
    pub fn remove_device(&mut self, device_node: String) -> bool {
        if let Entry::Occupied(occupied_entry) = self.cancellation_tokens.entry(device_node) {
            // Note: the message in spawn_serial is logged first, but with a MAC address. We do not
            // log anything here.
            occupied_entry.get().cancel();
            occupied_entry.remove();
            true
        } else {
            false
        }
    }

    /// Spawns a task to read messages from a Meshtastic serial connection.
    ///
    /// The task forwards the messages to the message handler.
    fn spawn_serial(&mut self, mut meshtastic_serial: MeshtasticSerial) -> CancellationToken {
        let mac_address = meshtastic_serial.mac_address();
        let cancellation_token = CancellationToken::new();
        let cancellation_token_clone = cancellation_token.clone();
        let mesh_proto_tx = self.mesh_proto_tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = meshtastic_serial.next_message() => {
                        match msg {
                            MeshProto::MeshPacket(mesh_packet) => {
                                mesh_proto_tx.send((mesh_packet, mac_address))
                                    .expect("Channel unexpectedly closed");
                            }
                            MeshProto::Disconnected(_device_node) => {
                                warn!("Removed meshtastic device: {mac_address}");
                                cancellation_token.cancel();
                            }
                        }
                    }
                    _ = cancellation_token.cancelled() => {
                        break;
                    }
                }
            }
        });
        cancellation_token_clone
    }
}

impl MessageHandler {
    /// Creates a new `MessageHandler`.
    ///
    /// This function initializes the `FleetState` and an optional `MqttReceiver`.
    pub fn new(dns: Vec<(String, MacAddress)>, mqtt_config: Option<MqttConfig>) -> Self {
        let macs = dns.iter().map(|(_, mac)| mac);
        let mqtt_receiver = mqtt_config.map(|config| MqttReceiver::new(config, macs));
        // let (dev_tx, dev_rx) = unbounded_channel();
        let (mesh_proto_tx, mesh_proto_rx) = unbounded_channel::<(MeshPacket, MacAddress)>();
        Self {
            fleet_state: FleetState::new(dns, Duration::from_secs(60)),
            mqtt_receiver,
            mesh_proto_tx,
            mesh_proto_rx,
        }
    }

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

    pub fn meshtastic_device_handler(&self) -> MshDevHandler {
        MshDevHandler {
            mesh_proto_tx: self.mesh_proto_tx.clone(),
            cancellation_tokens: HashMap::new(),
        }
    }
}
