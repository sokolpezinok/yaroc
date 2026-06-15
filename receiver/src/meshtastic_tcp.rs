use log::{error, info, warn};
use meshtastic::protobufs::MeshPacket;
use meshtastic::utils::stream;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

use crate::meshtastic_connection::MeshtasticConnection;
use crate::system_info::MacAddress;

pub struct MeshtasticTcp {
    host: String,
    connection: MeshtasticConnection,
}

impl MeshtasticTcp {
    /// Connects to a Meshtastic device over TCP.
    pub async fn connect(
        host: &str,
        timeout: Duration,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let tcp_stream = stream::build_tcp_stream(host.to_owned()).await.map_err(
            |err| -> Box<dyn std::error::Error + Send + Sync> { err.to_string().into() },
        )?;
        let connection = MeshtasticConnection::connect_stream(tcp_stream, timeout).await?;
        info!(
            "Connected to Meshtastic TCP device: {} at {}",
            connection.mac_address, host
        );

        Ok(Self {
            host: host.to_owned(),
            connection,
        })
    }

    /// Returns the MAC address of the device.
    pub fn mac_address(&self) -> MacAddress {
        self.connection.mac_address
    }

    /// An inner loop that reads messages from the Meshtastic device and sends them to a channel.
    pub async fn inner_loop(self, mesh_packet_tx: UnboundedSender<(MeshPacket, MacAddress)>) {
        self.connection.inner_loop(mesh_packet_tx, &self.host).await;
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
    use crate::meshtastic_connection::MeshtasticEvent;
    use meshtastic::protobufs::{FromRadio, MyNodeInfo, from_radio};
    use prost::Message;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

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

            // 3. Send a Channel settings packet
            let channel_settings = meshtastic::protobufs::ChannelSettings {
                name: "test_channel".to_owned(),
                ..Default::default()
            };
            let channel_info = meshtastic::protobufs::Channel {
                role: meshtastic::protobufs::channel::Role::Primary.into(),
                settings: Some(channel_settings),
                ..Default::default()
            };
            let from_radio_channel = FromRadio {
                payload_variant: Some(from_radio::PayloadVariant::Channel(channel_info)),
                ..Default::default()
            };
            let buf3 = encode_from_radio(from_radio_channel);
            socket.write_all(&buf3).await.unwrap();
        });

        // Connect client
        let mut client =
            MeshtasticTcp::connect(&addr.to_string(), Duration::from_secs(2)).await.unwrap();
        assert_eq!(client.mac_address(), MacAddress::Meshtastic(42));
        assert!(client.connection.channels.is_empty());

        let event = client.connection.next_message().await;
        let MeshtasticEvent::MeshPacket(packet) = event else {
            panic!("Expected MeshPacket");
        };
        assert_eq!(packet.from, 43);
        assert_eq!(packet.to, 42);

        // Next message should process the channel packet, push it to channels, and then wait for next.
        // Since the mock server closes the socket after sending the channel packet, this next_message()
        // call will return MeshtasticEvent::Disconnected.
        let event = client.connection.next_message().await;
        assert_eq!(event, MeshtasticEvent::Disconnected);

        // Verify that the channel name was indeed stored
        assert_eq!(client.connection.channels, vec!["test_channel".to_string()]);
    }
}
