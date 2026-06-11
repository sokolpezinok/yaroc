use std::time::Duration;

use crate::system_info::MacAddress;
use chrono::{DateTime, Local};
use femtopb::Message as _;
use log::{error, info, warn};
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, Publish, QoS};
use uuid::Uuid;
use yaroc_common::{
    error::Error,
    proto::{Disconnected, Status, status::Msg},
};

/// Configuration options for connecting to an MQTT broker.
pub struct MqttConfig {
    /// The URL/host of the MQTT broker.
    pub url: String,
    /// The port to connect to on the MQTT broker.
    pub port: u16,
    /// Optional username and password credentials.
    pub credentials: Option<(String, String)>,
    /// Keep-alive duration for the MQTT connection.
    pub keep_alive: Duration,
    /// Optional Meshtastic channel name to subscribe to.
    pub meshtastic_channel: Option<String>,
}

impl Default for MqttConfig {
    /// Returns the default MQTT configuration pointing to the EMQX public broker.
    fn default() -> Self {
        Self {
            url: "broker.emqx.io".to_owned(),
            port: 1883,
            credentials: None,
            keep_alive: Duration::from_secs(15),
            meshtastic_channel: None,
        }
    }
}

impl MqttConfig {
    /// Sets the username and password credentials for the connection.
    pub fn set_credentials(&mut self, username: &str, password: &str) {
        self.credentials = Some((username.to_owned(), password.to_owned()));
    }
}

/// An MQTT receiver that handles subscriptions and listens for incoming messages.
pub struct MqttReceiver {
    client: AsyncClient,
    topics: Vec<String>,
    event_loop: EventLoop,
    url: String,
    #[cfg(test)]
    /// An optional fixed timestamp to override `Local::now()` for deterministic testing.
    test_now: Option<DateTime<Local>>,
}

/// Represents messages received from MQTT topics.
#[derive(Debug, PartialEq, Eq)]
pub enum Message {
    /// Cellular status update containing the sender MAC, arrival timestamp, and raw payload.
    CellularStatus(MacAddress, DateTime<Local>, Vec<u8>),
    /// Punches data containing the sender MAC, arrival timestamp, and raw payload.
    Punches(MacAddress, DateTime<Local>, Vec<u8>),
    /// Meshtastic raw serial message with arrival timestamp and raw payload.
    MeshtasticSerial(DateTime<Local>, Vec<u8>),
    /// Meshtastic status update containing the sender MAC, arrival timestamp, and raw payload.
    MeshtasticStatus(MacAddress, DateTime<Local>, Vec<u8>),
}

impl MqttReceiver {
    /// Creates a new `MqttReceiver` instance by connecting to the broker specified in `config`
    /// and subscribing to topics corresponding to the provided MAC addresses.
    pub fn new<'a, I: Iterator<Item = &'a MacAddress>>(config: MqttConfig, macs: I) -> Self {
        let client_id = format!("yaroc-{}", Uuid::new_v4());
        let mut mqttoptions = MqttOptions::new(client_id, &config.url, config.port);
        mqttoptions.set_keep_alive(config.keep_alive);
        if let Some((username, password)) = config.credentials {
            mqttoptions.set_credentials(username, password);
        }

        let (client, event_loop) = AsyncClient::new(mqttoptions, 128);
        let mut topics = Vec::new();
        for mac in macs {
            if mac.is_full() {
                topics.push(format!("yar/{mac}/status"));
                topics.push(format!("yar/{mac}/p"));
                topics.push(format!("yar/{mac}/will"));
            } else {
                topics.push(format!("yar/2/e/serial/!{mac}"));
                if let Some(meshtastic_channel) = config.meshtastic_channel.as_ref() {
                    topics.push(format!("yar/2/e/{meshtastic_channel}/!{mac}"));
                }
            }
        }

        Self {
            url: config.url,
            client,
            event_loop,
            topics,
            #[cfg(test)]
            test_now: None,
        }
    }

    /// Extracts the cellular MAC address from the topic name.
    fn extract_cell_mac(topic: &str) -> crate::Result<MacAddress> {
        if topic.len() < 16 {
            return Err(Error::ParseError.into());
        }
        MacAddress::try_from(&topic[4..16])
    }

    /// Extracts the Meshtastic node MAC address from the topic name.
    fn extract_msh_mac(topic: &str) -> crate::Result<MacAddress> {
        if topic.len() <= 10 {
            // 8 for MAC, 2 for '/!'
            return Err(Error::ParseError.into());
        }
        MacAddress::try_from(&topic[topic.len() - 8..])
    }

    /// Processes an incoming MQTT message payload and topic, converting it into a structured `Message`.
    fn process_incoming(
        now: DateTime<Local>,
        topic: &str,
        payload: &[u8],
    ) -> crate::Result<Message> {
        if let Some(topic) = topic.strip_prefix("yar/2/e/") {
            let channel = topic.split_once("/");
            match channel {
                Some(("serial", _)) => Ok(Message::MeshtasticSerial(now, payload.into())),
                _ => {
                    let recv_mac_address = Self::extract_msh_mac(topic)?;
                    Ok(Message::MeshtasticStatus(
                        recv_mac_address,
                        now,
                        payload.into(),
                    ))
                }
            }
        } else {
            let mac_address = Self::extract_cell_mac(topic)?;
            match &topic[16..] {
                "/status" => Ok(Message::CellularStatus(mac_address, now, payload.into())),
                "/p" => Ok(Message::Punches(mac_address, now, payload.into())),
                "/will" => {
                    let status = Status {
                        msg: Some(Msg::Disconnected(Disconnected {
                            client_name: str::from_utf8(payload).map_err(|_| Error::ParseError)?,
                            ..Default::default()
                        })),
                        ..Default::default()
                    };
                    let mut payload = vec![0; status.encoded_len()];
                    status.encode(&mut payload.as_mut_slice()).unwrap();
                    Ok(Message::CellularStatus(mac_address, now, payload))
                }
                _ => Err(Error::ValueError.into()),
            }
        }
    }

    /// Retrieves the next incoming MQTT message by polling the event loop.
    /// Reconnects and resubscribes automatically upon connection errors.
    pub async fn next_message(&mut self) -> crate::Result<Message> {
        loop {
            let Ok(notification) = self.event_loop.poll().await else {
                error!("MQTT Connection error");
                // TODO: consider also exponential backoff up to the "keep alive" value.
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            };

            let now = {
                #[cfg(test)]
                {
                    self.test_now.unwrap_or_else(Local::now)
                }
                #[cfg(not(test))]
                {
                    Local::now()
                }
            };
            match notification {
                Event::Incoming(Packet::Publish(Publish { payload, topic, .. })) => {
                    return Self::process_incoming(now, &topic, &payload);
                }
                Event::Incoming(Packet::Disconnect) => {
                    warn!("MQTT Disconnected");
                }
                Event::Incoming(Packet::ConnAck(_)) => {
                    info!("Connected to MQTT server {}", self.url);
                    for topic in &self.topics {
                        self.client.subscribe(topic, QoS::AtMostOnce).await.unwrap();
                    }
                }
                _ => {} // ignored
            }
        }
    }

    /// Returns the URL of the MQTT broker this receiver is connected to.
    pub fn url(&self) -> &str {
        &self.url
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn test_new() {
        let macs = [
            MacAddress::Meshtastic(0x12345678),
            MacAddress::Full(0xdeadbeef9876),
        ];
        let config = MqttConfig {
            meshtastic_channel: Some("cha".to_owned()),
            ..Default::default()
        };
        let receiver = MqttReceiver::new(config, macs.iter());
        assert_eq!(
            receiver.topics,
            vec![
                "yar/2/e/serial/!12345678",
                "yar/2/e/cha/!12345678",
                "yar/deadbeef9876/status",
                "yar/deadbeef9876/p",
                "yar/deadbeef9876/will",
            ]
        );
    }

    #[test]
    fn test_new_without_msh() {
        let macs = [MacAddress::Meshtastic(0x12345678)];
        let config = MqttConfig::default();
        let receiver = MqttReceiver::new(config, macs.iter());
        assert_eq!(receiver.topics, vec!["yar/2/e/serial/!12345678"]);
    }

    #[test]
    fn test_extract_cell_mac() {
        let mac_address = MqttReceiver::extract_cell_mac("yar/deadbeef9876/p").unwrap();
        assert_eq!("deadbeef9876", format!("{mac_address}"));
        assert!(MqttReceiver::extract_cell_mac("yar/deadbeef987").is_err());
    }

    #[test]
    fn test_extract_msh_mac() {
        let mac_address = MqttReceiver::extract_msh_mac("cha/!12345678").unwrap();
        assert_eq!("12345678", format!("{mac_address}"));
        assert!(MqttReceiver::extract_cell_mac("cha/!1234567").is_err());
        assert!(MqttReceiver::extract_cell_mac("!12345678").is_err());
    }

    #[test]
    fn test_process_incoming() {
        let now = Local::now();

        let msg =
            MqttReceiver::process_incoming(now, "yar/deadbeef9876/status", b"hello cell").unwrap();
        assert_eq!(
            msg,
            Message::CellularStatus(
                MacAddress::try_from("deadbeef9876").unwrap(),
                now,
                "hello cell".into()
            )
        );

        let msg =
            MqttReceiver::process_incoming(now, "yar/deadbeef9876/p", b"hello punch").unwrap();
        assert_eq!(
            msg,
            Message::Punches(
                MacAddress::try_from("deadbeef9876").unwrap(),
                now,
                "hello punch".into()
            )
        );

        let msg = MqttReceiver::process_incoming(now, "yar/2/e/serial/!12345678", b"hello serial")
            .unwrap();
        assert_eq!(msg, Message::MeshtasticSerial(now, "hello serial".into()));

        let msg = MqttReceiver::process_incoming(
            now,
            "yar/2/e/some_channel/!12345678",
            b"hello meshtastic",
        )
        .unwrap();
        assert_eq!(
            msg,
            Message::MeshtasticStatus(
                MacAddress::try_from("12345678").unwrap(),
                now,
                "hello meshtastic".into()
            )
        );
    }

    /// Encodes an MQTT v3.1.1 (v3) QoS 0 PUBLISH packet with the given topic and payload.
    /// Used by the mock broker to send simulated messages to the client.
    fn encode_publish(topic: &str, payload: &[u8]) -> Vec<u8> {
        let topic_bytes = topic.as_bytes();
        let topic_len = topic_bytes.len();

        let var_header_len = 2 + topic_len;
        let remaining_len = var_header_len + payload.len();

        let mut packet = Vec::new();
        packet.push(0x30); // Control packet type PUBLISH, QoS 0

        // Encode remaining length (variable length byte representation)
        let mut val = remaining_len;
        loop {
            let mut byte = (val & 127) as u8;
            val /= 128;
            if val > 0 {
                byte |= 128;
            }
            packet.push(byte);
            if val == 0 {
                break;
            }
        }

        packet.push((topic_len >> 8) as u8);
        packet.push((topic_len & 0xFF) as u8);
        packet.extend_from_slice(topic_bytes);
        packet.extend_from_slice(payload);
        packet
    }

    #[tokio::test]
    async fn test_next_message_success() {
        // Bind TcpListener to a random port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Spawn a background task representing the mock MQTT broker
        let broker_handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();

            // 1. Read CONNECT packet
            let mut buf = [0u8; 1024];
            let _n = socket.read(&mut buf).await.unwrap();

            // 2. Write CONNACK packet
            socket.write_all(&[0x20, 0x02, 0x00, 0x00]).await.unwrap();

            // 3. Read SUBSCRIBE packet
            let _n = socket.read(&mut buf).await.unwrap();

            // 4. Write PUBLISH packet
            // Topic: yar/deadbeef9876/status
            // Payload: cellular status payload
            let topic = "yar/deadbeef9876/status";
            let payload = b"cellular status payload";
            let publish_packet = encode_publish(topic, payload);
            socket.write_all(&publish_packet).await.unwrap();
            tokio::time::sleep(Duration::from_millis(500)).await;
        });

        // Setup MqttReceiver pointing to our mock broker
        let macs = [MacAddress::Full(0xdeadbeef9876)];
        let config = MqttConfig {
            url: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let mut receiver = MqttReceiver::new(config, macs.iter());
        let exact_time = Local::now();
        receiver.test_now = Some(exact_time);

        // Get the next message with a timeout to prevent hanging
        let result = tokio::time::timeout(Duration::from_secs(1), receiver.next_message())
            .await
            .expect("MQTT next_message() test timed out");
        assert_eq!(
            result.unwrap(),
            Message::CellularStatus(macs[0], exact_time, b"cellular status payload".into())
        );

        broker_handle.await.unwrap();
    }
}
