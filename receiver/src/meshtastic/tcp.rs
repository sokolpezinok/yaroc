use futures::{Stream, StreamExt};
use log::{error, info};
use meshtastic::protobufs::ServiceEnvelope;
use meshtastic::utils::stream;
use std::time::Duration;

use super::connection::MeshtasticConnection;

/// Connects to a Meshtastic device over TCP.
async fn connect(
    host: &str,
    timeout: Duration,
) -> Result<MeshtasticConnection, Box<dyn std::error::Error + Send + Sync>> {
    let tcp_stream = stream::build_tcp_stream(host.to_owned()).await?;
    let connection =
        MeshtasticConnection::connect_stream(tcp_stream, timeout, host.to_owned()).await?;
    info!(
        "Connected to Meshtastic TCP device: {} at {}",
        connection.mac_address, host
    );

    Ok(connection)
}

/// Connects to a Meshtastic device over TCP and returns a stream of `ServiceEnvelope` messages,
/// automatically reconnecting if disconnected or failed.
pub fn into_stream(host: String) -> impl Stream<Item = ServiceEnvelope> + Send {
    let connect_timeout = Duration::from_secs(5);
    futures::stream::unfold(host, move |host| async move {
        loop {
            match connect(&host, connect_timeout).await {
                Ok(meshtastic_tcp) => {
                    return Some((meshtastic_tcp.into_stream(), host));
                }
                Err(err) => {
                    error!(
                        "Failed to connect to Meshtastic TCP device at {host}: {err}. Retrying in 5 seconds..."
                    );
                    tokio::time::sleep(connect_timeout).await;
                }
            }
        }
    })
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system_info::MacAddress;
    use crate::test_utils::encode_from_radio;
    use meshtastic::protobufs::{FromRadio, MeshPacket, MyNodeInfo, from_radio};
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

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
        let mut connection = connect(&addr.to_string(), Duration::from_secs(2)).await.unwrap();
        assert_eq!(connection.mac_address, MacAddress::Meshtastic(42));
        assert!(connection.channels.is_empty());

        let packet = connection.next_message().await.expect("Expected MeshPacket");
        assert_eq!(packet.from, 43);
        assert_eq!(packet.to, 42);

        // Next message should process the channel packet, push it to channels, and then wait for next.
        // Since the mock server closes the socket after sending the channel packet, this next_message()
        // call will return None.
        assert!(connection.next_message().await.is_none());

        // Verify that the channel name was indeed stored
        assert_eq!(connection.channels, vec!["test_channel".to_string()]);
    }
}
