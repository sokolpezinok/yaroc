use log::{error, info, warn};
use meshtastic::api::{ConnectedStreamApi, StreamApi};
use meshtastic::protobufs::{FromRadio, MeshPacket, from_radio};
use meshtastic::utils;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::Instant;
use tokio_util::future::FutureExt as _;

use crate::error::Error;
use crate::system_info::MacAddress;

pub struct MeshtasticTcp {
    host: String,
    stream_api: ConnectedStreamApi,
    listener: tokio::sync::mpsc::UnboundedReceiver<FromRadio>,
    mac_address: MacAddress,
}

impl MeshtasticTcp {
    /// Connects to a Meshtastic device over TCP.
    pub async fn connect(
        host: &str,
        timeout: Duration,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let tcp_stream = utils::stream::build_tcp_stream(host.to_owned()).await.map_err(
            |err| -> Box<dyn std::error::Error + Send + Sync> { err.to_string().into() },
        )?;

        let deadline = Instant::now() + timeout;
        let stream_api = StreamApi::new();
        let (mut listener, stream_api) =
            stream_api.connect(tcp_stream).timeout_at(deadline).await?;

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
            host: host.to_owned(),
            stream_api,
            listener,
            mac_address: MacAddress::Meshtastic(my_node_info.my_node_num),
        })
    }

    /// Returns the MAC address of the device.
    pub fn mac_address(&self) -> MacAddress {
        self.mac_address
    }

    /// An inner loop that reads messages from the Meshtastic device and sends them to a channel.
    pub async fn inner_loop(mut self, mesh_packet_tx: UnboundedSender<(MeshPacket, MacAddress)>) {
        info!(
            "Connected to Meshtastic TCP device: {} at {}",
            self.mac_address, self.host
        );
        loop {
            match self.listener.recv().await {
                Some(FromRadio {
                    payload_variant: Some(from_radio::PayloadVariant::Packet(packet)),
                    ..
                }) => {
                    if let Err(err) = mesh_packet_tx.send((packet, self.mac_address)) {
                        error!("Failed to forward packet: {err}");
                        break;
                    }
                }
                None => {
                    warn!("Disconnected from Meshtastic TCP device: {}", self.host);
                    let _ = self.stream_api.disconnect().await;
                    break;
                }
                _ => {}
            }
        }
    }
}

/// Connects to a Meshtastic device over TCP and handles automatic reconnection if disconnected or failed.
pub async fn connect_and_loop(
    host: String,
    mesh_packet_tx: UnboundedSender<(MeshPacket, MacAddress)>,
) {
    let connect_timeout = Duration::from_secs(5);
    loop {
        match MeshtasticTcp::connect(&host, connect_timeout).await {
            Ok(meshtastic_tcp) => {
                meshtastic_tcp.inner_loop(mesh_packet_tx.clone()).await;
                warn!(
                    "Disconnected from Meshtastic TCP device at {host}. Retrying in 5 seconds..."
                );
            }
            Err(err) => {
                error!(
                    "Failed to connect to Meshtastic TCP device at {host}: {err}. Retrying in 5 seconds..."
                );
            }
        }
        tokio::time::sleep(connect_timeout).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshtastic::protobufs::MyNodeInfo;
    use prost::Message;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    fn encode_from_radio(msg: FromRadio) -> Vec<u8> {
        let mut protobuf_bytes = Vec::new();
        msg.encode(&mut protobuf_bytes).unwrap();
        let size = protobuf_bytes.len() as u16;
        let size_bytes = size.to_be_bytes();
        let mut header = vec![0x94, 0xc3, size_bytes[0], size_bytes[1]];
        header.extend_from_slice(&protobuf_bytes);
        header
    }

    #[tokio::test]
    async fn test_meshtastic_tcp_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();

            // 1. Send MyInfo packet first
            let my_info = MyNodeInfo {
                my_node_num: 42,
                ..Default::default()
            };
            let from_radio_info = FromRadio {
                payload_variant: Some(from_radio::PayloadVariant::MyInfo(my_info)),
                ..Default::default()
            };
            let buf = encode_from_radio(from_radio_info);
            socket.write_all(&buf).await.unwrap();

            // 2. Send a MeshPacket
            let mesh_packet = MeshPacket {
                from: 43,
                to: 42,
                ..Default::default()
            };
            let from_radio_packet = FromRadio {
                payload_variant: Some(from_radio::PayloadVariant::Packet(mesh_packet)),
                ..Default::default()
            };
            let buf2 = encode_from_radio(from_radio_packet);
            socket.write_all(&buf2).await.unwrap();
        });

        // Connect client
        let client =
            MeshtasticTcp::connect(&addr.to_string(), Duration::from_secs(2)).await.unwrap();
        assert_eq!(client.mac_address(), MacAddress::Meshtastic(42));

        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            client.inner_loop(tx).await;
        });

        let (received_packet, mac) = rx.recv().await.unwrap();
        assert_eq!(received_packet.from, 43);
        assert_eq!(received_packet.to, 42);
        assert_eq!(mac, MacAddress::Meshtastic(42));
    }
}
