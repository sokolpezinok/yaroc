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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::encode_from_radio;
    use meshtastic::{
        api::StreamHandle,
        protobufs::{FromRadio, MeshPacket, MyNodeInfo, from_radio},
    };
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_meshtastic_connection_chunked_bytes() {
        let (client_stream, mut server_stream) = tokio::io::duplex(1024);

        tokio::spawn(async move {
            let my_info = MyNodeInfo {
                my_node_num: 100,
                ..Default::default()
            };
            let from_radio_info = FromRadio {
                payload_variant: Some(from_radio::PayloadVariant::MyInfo(my_info)),
                ..Default::default()
            };
            let buf = encode_from_radio(from_radio_info);

            // Write payload byte-by-byte to test chunked AsyncRead framing
            for byte in buf {
                server_stream.write_all(&[byte]).await.unwrap();
                tokio::task::yield_now().await;
            }

            let mesh_packet = MeshPacket {
                from: 200,
                to: 100,
                ..Default::default()
            };
            let from_radio_packet = FromRadio {
                payload_variant: Some(from_radio::PayloadVariant::Packet(mesh_packet)),
                ..Default::default()
            };
            let buf2 = encode_from_radio(from_radio_packet);

            // Write second packet in small 2-byte chunks
            for chunk in buf2.chunks(2) {
                server_stream.write_all(chunk).await.unwrap();
                tokio::task::yield_now().await;
            }
        });

        let stream_handle = StreamHandle::from_stream(client_stream);
        let mut connection =
            MeshtasticConnection::connect_stream(stream_handle, Duration::from_secs(2))
                .await
                .unwrap();

        assert_eq!(connection.mac_address, MacAddress::Meshtastic(100));

        let event = connection.next_message().await;
        let MeshtasticEvent::MeshPacket(packet) = event else {
            panic!("Expected MeshPacket");
        };
        assert_eq!(packet.from, 200);
        assert_eq!(packet.to, 100);
    }

    #[tokio::test]
    async fn test_meshtastic_connection_closed_before_config() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        drop(server_stream); // Close stream immediately

        let stream_handle = StreamHandle::from_stream(client_stream);
        let res = MeshtasticConnection::connect_stream(stream_handle, Duration::from_secs(2)).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_meshtastic_connection_invalid_handshake() {
        let (client_stream, mut server_stream) = tokio::io::duplex(1024);

        tokio::spawn(async move {
            // Send MeshPacket instead of MyNodeInfo handshake
            let mesh_packet = MeshPacket {
                from: 43,
                to: 42,
                ..Default::default()
            };
            let from_radio_packet = FromRadio {
                payload_variant: Some(from_radio::PayloadVariant::Packet(mesh_packet)),
                ..Default::default()
            };
            let buf = encode_from_radio(from_radio_packet);
            server_stream.write_all(&buf).await.unwrap();
        });

        let stream_handle = StreamHandle::from_stream(client_stream);
        let res = MeshtasticConnection::connect_stream(stream_handle, Duration::from_secs(2)).await;
        assert!(res.is_err());
    }
}
