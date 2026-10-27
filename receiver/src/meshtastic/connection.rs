use std::time::Duration;

use futures::StreamExt;
use log::{error, warn};
use meshtastic::api::{ConnectedStreamApi, StreamApi};
use meshtastic::protobufs::{FromRadio, MeshPacket, ServiceEnvelope, channel, from_radio};
use meshtastic::utils;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::error::Error;
use crate::system_info::MacAddress;

/// A connection to a Meshtastic device, wrapping both the stream API and the packet listener.
pub struct MeshtasticConnection {
    pub stream_api: ConnectedStreamApi,
    pub listener: UnboundedReceiver<FromRadio>,
    pub mac_address: MacAddress,
    pub channels: Vec<String>,
    pub device_name: String,
}

impl MeshtasticConnection {
    /// Creates a new Meshtastic connection using a provided stream handle.
    pub async fn connect_stream<S>(
        stream: meshtastic::api::StreamHandle<S>,
        timeout: Duration,
        device_name: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        tokio::time::timeout(timeout, async {
            let stream_api = StreamApi::new();
            let (mut listener, stream_api) = stream_api.connect(stream).await;
            let stream_api = stream_api.configure(utils::generate_rand_id()).await?;

            let packet = listener.recv().await.ok_or("Stream closed before configuration")?;
            let FromRadio {
                payload_variant: Some(from_radio::PayloadVariant::MyInfo(my_node_info)),
                ..
            } = packet
            else {
                return Err(Error::ConnectionError.into());
            };

            Ok(Self {
                stream_api,
                listener,
                mac_address: MacAddress::Meshtastic(my_node_info.my_node_num),
                channels: Vec::new(),
                device_name,
            })
        })
        .await?
    }

    /// Waits for the next mesh packet from the device, or returns `None` if disconnected.
    pub async fn next_message(&mut self) -> Option<MeshPacket> {
        loop {
            match self.listener.recv().await?.payload_variant {
                Some(from_radio::PayloadVariant::Packet(packet)) => return Some(packet),
                Some(from_radio::PayloadVariant::Channel(channel)) => {
                    if channel.role != channel::Role::Disabled.into()
                        && let Some(settings) = channel.settings
                    {
                        self.channels.push(settings.name);
                    }
                }
                _ => {
                    // Unimportant packet, do nothing.
                }
            }
        }
    }

    /// Converts this connection into a stream of `ServiceEnvelope` messages.
    pub fn into_stream(self) -> impl futures::Stream<Item = ServiceEnvelope> + Send {
        futures::stream::unfold(self, |mut conn| async move {
            if let Some(mesh_packet) = conn.next_message().await {
                let service_envelope = ServiceEnvelope {
                    packet: Some(mesh_packet),
                    channel_id: conn.channels.first().cloned().unwrap_or_default(),
                    gateway_id: format!("!{}", conn.mac_address),
                };
                Some((service_envelope, conn))
            } else {
                warn!("Disconnected from Meshtastic device: {}", conn.device_name);
                let _ = conn.stream_api.disconnect().await;
                None
            }
        })
    }

    /// An inner loop that reads messages from the Meshtastic device and sends them to a channel.
    pub async fn inner_loop(self, mesh_packet_tx: UnboundedSender<ServiceEnvelope>) {
        let mut stream = std::pin::pin!(self.into_stream());
        while let Some(service_envelope) = stream.next().await {
            if let Err(err) = mesh_packet_tx.send(service_envelope) {
                error!("Failed to forward packet: {err}");
                break;
            }
        }
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
        let mut connection = MeshtasticConnection::connect_stream(
            stream_handle,
            Duration::from_secs(2),
            "test".to_owned(),
        )
        .await
        .unwrap();

        assert_eq!(connection.mac_address, MacAddress::Meshtastic(100));

        let packet = connection.next_message().await.expect("Expected MeshPacket");
        assert_eq!(packet.from, 200);
        assert_eq!(packet.to, 100);
    }

    #[tokio::test]
    async fn test_meshtastic_connection_closed_before_config() {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        drop(server_stream); // Close stream immediately

        let stream_handle = StreamHandle::from_stream(client_stream);
        let res = MeshtasticConnection::connect_stream(
            stream_handle,
            Duration::from_secs(2),
            "test".to_owned(),
        )
        .await;
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
        let res = MeshtasticConnection::connect_stream(
            stream_handle,
            Duration::from_secs(2),
            "test".to_owned(),
        )
        .await;
        assert!(res.is_err());
    }
}
