use std::time::Duration;

use crate::system_info::MacAddress;
use chrono::{DateTime, Local};
use log::{error, info, warn};
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, Publish, QoS};
use uuid::Uuid;
use yaroc_common::error::Error;

pub struct MqttConfig {
    pub url: String,
    pub port: u16,
    pub credentials: Option<(String, String)>,
    pub keep_alive: Duration,
    pub meshtastic_channel: Option<String>,
}

impl Default for MqttConfig {
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
    pub fn set_credentials(&mut self, username: &str, password: &str) {
        self.credentials = Some((username.to_owned(), password.to_owned()));
    }
}

pub struct MqttReceiver {
    client: AsyncClient,
    topics: Vec<String>,
    event_loop: EventLoop,
    url: String,
}

#[derive(Debug)]
pub enum Message {
    CellularStatus(MacAddress, DateTime<Local>, Vec<u8>),
    Punches(MacAddress, DateTime<Local>, Vec<u8>),
    MeshtasticSerial(DateTime<Local>, Vec<u8>),
    MeshtasticStatus(MacAddress, DateTime<Local>, Vec<u8>),
}

impl MqttReceiver {
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
        }
    }

    fn extract_cell_mac(topic: &str) -> crate::Result<MacAddress> {
        if topic.len() < 16 {
            return Err(Error::ParseError.into());
        }
        MacAddress::try_from(&topic[4..16])
    }

    fn extract_msh_mac(topic: &str) -> crate::Result<MacAddress> {
        if topic.len() <= 10 {
            // 8 for MAC, 2 for '/!'
            return Err(Error::ParseError.into());
        }
        MacAddress::try_from(&topic[topic.len() - 8..])
    }

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
                _ => Err(Error::ValueError.into()),
            }
        }
    }

    pub async fn next_message(&mut self) -> crate::Result<Message> {
        loop {
            let Ok(notification) = self.event_loop.poll().await else {
                error!("MQTT Connection error");
                // TODO: consider also exponential backoff up to the "keep alive" value.
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            };

            let now = Local::now();
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
}

#[cfg(test)]
mod test {
    use super::*;

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
}
