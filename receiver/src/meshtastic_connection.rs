use std::time::Duration;

use log::{error, warn};
use meshtastic::api::{ConnectedStreamApi, StreamApi};
use meshtastic::protobufs::{FromRadio, MeshPacket, ServiceEnvelope, channel, from_radio};
use meshtastic::utils;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::Instant;
use tokio_util::future::FutureExt as _;

use crate::error::Error;
use crate::system_info::MacAddress;

/// An enum representing a message from a Meshtastic device.
#[derive(Debug, Clone, PartialEq)]
pub enum MeshtasticEvent {
    /// A mesh packet.
    MeshPacket(MeshPacket),
    /// The device was disconnected.
    Disconnected,
}

/// A connection to a Meshtastic device, wrapping both the stream API and the packet listener.
pub struct MeshtasticConnection {
    pub stream_api: ConnectedStreamApi,
    pub listener: UnboundedReceiver<FromRadio>,
    pub mac_address: MacAddress,
    pub channels: Vec<String>,
}

impl MeshtasticConnection {
    /// Creates a new Meshtastic connection using a provided stream handle.
    pub async fn connect_stream<S>(
        stream: meshtastic::api::StreamHandle<S>,
        timeout: Duration,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        let deadline = Instant::now() + timeout;
        let stream_api = StreamApi::new();
        let (mut listener, stream_api) = stream_api.connect(stream).timeout_at(deadline).await?;
        let config_id = utils::generate_rand_id();
        let stream_api = stream_api.configure(config_id).await?;

        let packet = listener.recv().timeout_at(deadline).await?.ok_or_else(
            || -> Box<dyn std::error::Error + Send + Sync> {
                "Stream closed before configuration".into()
            },
        )?;
        let FromRadio {
            payload_variant: Some(from_radio::PayloadVariant::MyInfo(my_node_info)),
            ..
        } = packet
        else {
            return Err(Box::new(Error::ConnectionError));
        };

        Ok(Self {
            stream_api,
            listener,
            mac_address: MacAddress::Meshtastic(my_node_info.my_node_num),
            channels: Vec::new(),
        })
    }

    /// Waits for the next message from the device.
    pub async fn next_message(&mut self) -> MeshtasticEvent {
        loop {
            match self.listener.recv().await {
                Some(FromRadio {
                    payload_variant: Some(from_radio::PayloadVariant::Packet(packet)),
                    ..
                }) => {
                    return MeshtasticEvent::MeshPacket(packet);
                }
                Some(FromRadio {
                    payload_variant: Some(from_radio::PayloadVariant::Channel(channel)),
                    ..
                }) => {
                    if channel.role != channel::Role::Disabled.into()
                        && let Some(settings) = channel.settings
                    {
                        self.channels.push(settings.name);
                    }
                }
                None => {
                    return MeshtasticEvent::Disconnected;
                }
                _ => {
                    // Unimportant packet, do nothing.
                }
            }
        }
    }

    /// An inner loop that reads messages from the Meshtastic device and sends them to a channel.
    pub async fn inner_loop(
        mut self,
        mesh_packet_tx: tokio::sync::mpsc::UnboundedSender<ServiceEnvelope>,
        device_name: &str,
    ) {
        loop {
            let event = self.next_message().await;
            match event {
                MeshtasticEvent::MeshPacket(mesh_packet) => {
                    let service_envelope = ServiceEnvelope {
                        packet: Some(mesh_packet),
                        channel_id: self.channels.first().cloned().unwrap_or_default(),
                        gateway_id: format!("!{}", self.mac_address),
                    };
                    if let Err(err) = mesh_packet_tx.send(service_envelope) {
                        error!("Failed to forward packet: {err}");
                        break;
                    }
                }
                MeshtasticEvent::Disconnected => {
                    warn!("Disconnected from Meshtastic device: {}", device_name);
                    break;
                }
            }
        }
        let _ = self.stream_api.disconnect().await;
    }
}
