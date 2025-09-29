use std::collections::HashMap;
use std::time::Duration;

use log::{error, info, warn};
use meshtastic::api::{ConnectedStreamApi, StreamApi};
use meshtastic::protobufs::{FromRadio, MeshPacket, from_radio};
use meshtastic::utils;
use std::collections::hash_map::Entry;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::{Instant, timeout_at};
use tokio_util::sync::CancellationToken;

use crate::error::Error;
use crate::system_info::MacAddress;

/// A connection to a Meshtastic device.
pub struct MeshtasticSerial {
    device_node: String,
    stream_api: ConnectedStreamApi,
    listener: UnboundedReceiver<FromRadio>,
    mac_address: MacAddress,
}

/// An enum representing a message from a Meshtastic device.
pub enum MeshtasticEvent {
    /// A mesh packet.
    MeshPacket(MeshPacket),
    /// The device was disconnected.
    Disconnected(String),
}

impl MeshtasticSerial {
    /// Creates a new Meshtastic serial connection.
    pub async fn new(
        port: &str,
        device_node: &str,
        timeout: Duration,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + timeout;
        let stream_api = StreamApi::new();
        let serial_stream = utils::stream::build_serial_stream(port.to_owned(), None, None, None)?;
        let (mut listener, stream_api) =
            timeout_at(deadline, stream_api.connect(serial_stream)).await?;
        let config_id = utils::generate_rand_id();
        let stream_api = stream_api.configure(config_id).await?;

        let packet = timeout_at(deadline, listener.recv()).await?;
        let Some(FromRadio {
            payload_variant: Some(from_radio::PayloadVariant::MyInfo(my_node_info)),
            ..
        }) = packet
        else {
            return Err(Box::new(Error::ConnectionError));
        };

        Ok(Self {
            device_node: device_node.to_owned(),
            stream_api,
            listener,
            mac_address: MacAddress::Meshtastic(my_node_info.my_node_num),
        })
    }

    /// Returns the device node.
    pub fn device_node(&self) -> &str {
        &self.device_node
    }

    /// Disconnects the Meshtastic device.
    pub async fn disconnect(self) -> Result<(), Box<dyn std::error::Error>> {
        self.stream_api.disconnect().await?;
        Ok(())
    }

    /// Returns the MAC address of the device.
    pub fn mac_address(&self) -> MacAddress {
        self.mac_address
    }

    /// Returns the next message from the device.
    pub async fn next_message(&mut self) -> MeshtasticEvent {
        loop {
            match self.listener.recv().await {
                Some(FromRadio {
                    payload_variant: Some(from_radio::PayloadVariant::Packet(packet)),
                    ..
                }) => {
                    return MeshtasticEvent::MeshPacket(packet);
                }
                None => {
                    return MeshtasticEvent::Disconnected(self.device_node.clone());
                }
                _ => {}
            }
        }
    }
}

/// Handles connecting and disconnecting of Meshtastic devices.
///
/// Currently, only serial port connections are supported.
/// Handles connecting and disconnecting of Meshtastic devices.
///
/// Currently, only serial port connections are supported.
pub struct MshDevHandler {
    cancellation_tokens: HashMap<String, CancellationToken>,
    mesh_proto_tx: UnboundedSender<(MeshPacket, MacAddress)>,
}

/// Meshtastic device handler
///
/// Handles connecting and disconnecting of meshtastic devices. Supports only serial port
/// connections right now.
impl MshDevHandler {
    /// Creates a new `MshDevHandler`.
    pub fn new(mesh_proto_tx: UnboundedSender<(MeshPacket, MacAddress)>) -> Self {
        Self {
            cancellation_tokens: HashMap::new(),
            mesh_proto_tx,
        }
    }
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
                            MeshtasticEvent::MeshPacket(mesh_packet) => {
                                mesh_proto_tx.send((mesh_packet, mac_address))
                                    .expect("Channel unexpectedly closed");
                            }
                            MeshtasticEvent::Disconnected(_device_node) => {
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
