use std::time::Duration;

use chrono::{DateTime, Local};
use femtopb::Message as _;
use futures::Stream;
use log::{error, info, warn};
use meshtastic::protobufs::ServiceEnvelope;
use prost::Message as _;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, Publish, QoS};
use uuid::Uuid;

use yaroc_common::{
    error::Error,
    proto::{Disconnected, Status, status::Msg},
};

use crate::meshtastic::{MESHTASTIC_MQTT_PREFIX, MeshtasticLog, SERIAL_APP};
use crate::system_info::MacAddress;

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

/// Represents messages received from MQTT topics.
#[derive(Debug, PartialEq, Eq)]
pub enum Message {
    /// Cellular status update containing the sender MAC, arrival timestamp, and raw payload.
    CellularStatus(MacAddress, DateTime<Local>, Vec<u8>),
    /// Punches data containing the sender MAC, arrival timestamp, and raw payload.
    Punches(MacAddress, DateTime<Local>, Vec<u8>),
    /// Meshtastic raw serial message with arrival timestamp and raw payload.
    MeshtasticSerial(MacAddress, DateTime<Local>, Vec<u8>),
    /// Meshtastic status update containing the sender MAC, arrival timestamp, and raw payload.
    MeshtasticStatus(MacAddress, DateTime<Local>, Vec<u8>),
}

impl Message {
    /// Returns the sender/gateway MAC address of this message.
    pub fn mac_address(&self) -> MacAddress {
        match self {
            Message::CellularStatus(mac, _, _)
            | Message::Punches(mac, _, _)
            | Message::MeshtasticSerial(mac, _, _)
            | Message::MeshtasticStatus(mac, _, _) => *mac,
        }
    }
}

/// Builds the list of MQTT topics to subscribe to for the given MAC addresses.
fn topics<'a, I: Iterator<Item = &'a MacAddress>>(
    macs: I,
    meshtastic_channel: Option<&str>,
) -> Vec<String> {
    let mut topics = Vec::new();
    for mac in macs {
        if mac.is_full() {
            topics.push(format!("yar/{mac}/status"));
            topics.push(format!("yar/{mac}/p"));
            topics.push(format!("yar/{mac}/will"));
        } else {
            topics.push(format!("yar/2/e/serial/!{mac}"));
            if let Some(meshtastic_channel) = meshtastic_channel {
                topics.push(format!("yar/2/e/{meshtastic_channel}/!{mac}"));
            }
        }
    }
    topics
}

/// An MQTT receiver that handles subscriptions and listens for incoming messages.
#[derive(Clone)]
pub struct MqttReceiver {
    client: AsyncClient,
}

/// Internal state for processing MQTT events in the incoming message stream.
struct MqttStreamHandler {
    client: AsyncClient,
    topics: Vec<String>,
    url: String,
    #[cfg(test)]
    /// An optional fixed timestamp to override `Local::now()` for deterministic testing.
    test_now: Option<DateTime<Local>>,
}

impl MqttStreamHandler {
    /// Processes rumqttc::Event.
    async fn process_mqtt_event(
        &mut self,
        event: rumqttc::Event,
    ) -> Option<crate::Result<Message>> {
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
        match event {
            Event::Incoming(Packet::Publish(Publish { payload, topic, .. })) => {
                return Some(MqttReceiver::process_incoming(now, &topic, &payload));
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
            _ => {} // Ignored
        }
        None
    }
}

impl MqttReceiver {
    /// Creates a new `MqttReceiver` instance and an incoming message stream by connecting to the
    /// broker specified in `config` and subscribing to topics corresponding to the provided MAC addresses.
    pub fn new<'a, I: Iterator<Item = &'a MacAddress>>(
        config: MqttConfig,
        macs: I,
    ) -> (
        Self,
        impl Stream<Item = crate::Result<Message>> + Send + 'static,
    ) {
        let client_id = format!("yaroc-{}", Uuid::new_v4());
        let mut mqttoptions = MqttOptions::new(client_id, &config.url, config.port);
        mqttoptions.set_keep_alive(config.keep_alive);
        if let Some((username, password)) = config.credentials {
            mqttoptions.set_credentials(username, password);
        }

        let (client, event_loop) = AsyncClient::new(mqttoptions, 128);
        let topics = topics(macs, config.meshtastic_channel.as_deref());

        let receiver = Self {
            client: client.clone(),
        };

        let stream_handler = MqttStreamHandler {
            client,
            url: config.url,
            topics,
            #[cfg(test)]
            test_now: None,
        };
        let stream = futures::stream::unfold(
            (event_loop, stream_handler),
            |(mut event_loop, mut handler)| async move {
                loop {
                    let Ok(event) = event_loop.poll().await else {
                        error!("MQTT Connection error");
                        // TODO: consider also exponential backoff up to the "keep alive" value.
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    };
                    if let Some(msg) = handler.process_mqtt_event(event).await {
                        return Some((msg, (event_loop, handler)));
                    }
                }
            },
        );

        (receiver, stream)
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
        if let Some(topic) = topic.strip_prefix(MESHTASTIC_MQTT_PREFIX) {
            let channel = topic.split_once("/");
            match channel {
                Some(("serial", _)) => {
                    let recv_mac_address = Self::extract_msh_mac(topic)?;
                    Ok(Message::MeshtasticSerial(
                        recv_mac_address,
                        now,
                        payload.into(),
                    ))
                }
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

    /// Publish meshtastic service envelope protobuf.
    pub async fn publish_meshtastic_service_envelope(
        &self,
        service_envelope: ServiceEnvelope,
    ) -> crate::Result<()> {
        let channel = if let Some(packet) = &service_envelope.packet
            && MeshtasticLog::get_mesh_packet_portnum(packet).is_ok_and(|p| p == SERIAL_APP)
        {
            "serial"
        } else {
            &service_envelope.channel_id
        };
        let topic = format!(
            "{MESHTASTIC_MQTT_PREFIX}{channel}/{}",
            service_envelope.gateway_id
        );
        let payload = service_envelope.encode_to_vec();
        self.client.publish(topic, QoS::AtMostOnce, false, payload).await?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_topics() {
        let macs = [
            MacAddress::Meshtastic(0x12345678),
            MacAddress::Full(0xdeadbeef9876),
        ];
        assert_eq!(
            topics(macs.iter(), Some("cha")),
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
    fn test_topics_without_msh() {
        let macs = [MacAddress::Meshtastic(0x12345678)];
        assert_eq!(
            topics(macs.iter(), None),
            vec!["yar/2/e/serial/!12345678"]
        );
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
        assert_eq!(
            msg,
            Message::MeshtasticSerial(
                MacAddress::try_from("12345678").unwrap(),
                now,
                "hello serial".into()
            )
        );

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

    #[tokio::test]
    async fn test_process_mqtt_event_publish() {
        let macs = [MacAddress::Full(0xdeadbeef9876)];
        let (client, _) = AsyncClient::new(MqttOptions::new("test", "localhost", 1883), 1);
        let exact_time = Local::now();
        let mut handler = MqttStreamHandler {
            client,
            topics: vec![],
            url: "localhost".to_string(),
            test_now: Some(exact_time),
        };

        let publish_event = Event::Incoming(Packet::Publish(Publish {
            dup: false,
            qos: QoS::AtMostOnce,
            retain: false,
            topic: "yar/deadbeef9876/status".to_string(),
            pkid: 0,
            payload: b"cellular status payload".to_vec().into(),
        }));

        let result = handler.process_mqtt_event(publish_event).await;
        assert_eq!(
            result.unwrap().unwrap(),
            Message::CellularStatus(macs[0], exact_time, b"cellular status payload".to_vec())
        );
    }

    #[tokio::test]
    async fn test_process_mqtt_event_disconnect_and_ignored() {
        let (client, _) = AsyncClient::new(MqttOptions::new("test", "localhost", 1883), 1);
        let mut handler = MqttStreamHandler {
            client,
            topics: vec![],
            url: "localhost".to_string(),
            test_now: None,
        };

        let disconnect_event = Event::Incoming(Packet::Disconnect);
        assert!(handler.process_mqtt_event(disconnect_event).await.is_none());

        let ping_event = Event::Incoming(Packet::PingResp);
        assert!(handler.process_mqtt_event(ping_event).await.is_none());
    }

    #[tokio::test]
    async fn test_publish_meshtastic_service_envelope() {
        let (receiver, _stream) = MqttReceiver::new(Default::default(), [].iter());
        let envelope = ServiceEnvelope {
            packet: None,
            channel_id: "test_chan".to_string(),
            gateway_id: "!12345678".to_string(),
        };
        assert!(receiver.publish_meshtastic_service_envelope(envelope).await.is_ok());
    }
}
