use std::fmt::Display;
use std::time::Duration;

use log::warn;
use meshtastic::api::{ConnectedStreamApi, StreamApi};
use meshtastic::protobufs::{FromRadio, MeshPacket, from_radio};
use meshtastic::utils;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::{Instant, timeout_at};

use crate::error::Error;
use crate::serial_device_manager::UsbSerialTrait;
use crate::system_info::MacAddress;

/// An enum representing a message from a Meshtastic device.
pub enum MeshtasticEvent {
    /// A mesh packet.
    MeshPacket(MeshPacket),
    /// The device was disconnected.
    Disconnected(String),
}

/// A connection to a Meshtastic device.
pub struct MeshtasticSerial {
    device_node: String,
    stream_api: ConnectedStreamApi,
    listener: UnboundedReceiver<FromRadio>,
    mac_address: MacAddress,
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

    /// Waits for the next message from the device.
    async fn next_message(&mut self) -> MeshtasticEvent {
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

    /// Returns the device node.
    pub fn device_node(&self) -> &str {
        &self.device_node
    }

    /// Returns the MAC address of the device.
    pub fn mac_address(&self) -> MacAddress {
        self.mac_address
    }

    /// Disconnects the Meshtastic device.
    pub async fn disconnect(self) -> Result<(), Box<dyn std::error::Error>> {
        self.stream_api.disconnect().await?;
        Ok(())
    }
}

impl UsbSerialTrait for MeshtasticSerial {
    type Output = (MeshPacket, MacAddress);

    /// An inner loop that reads messages from the Meshtastic device and sends them to a channel.
    async fn inner_loop(mut self, mesh_proto_tx: UnboundedSender<(MeshPacket, MacAddress)>) {
        loop {
            let event = self.next_message().await;
            match event {
                MeshtasticEvent::MeshPacket(mesh_packet) => {
                    mesh_proto_tx
                        .send((mesh_packet, self.mac_address))
                        .expect("Channel unexpectedly closed");
                }
                MeshtasticEvent::Disconnected(_device_node) => {
                    warn!("Removed meshtastic device: {}", self.mac_address);
                    break;
                }
            }
        }
    }
}

impl Display for MeshtasticSerial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Meshtastic device {}", self.mac_address)
    }
}
