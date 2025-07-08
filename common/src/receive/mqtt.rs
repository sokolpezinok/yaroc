extern crate std;

use crate::error::Error;
use crate::system_info::MacAddress;

use chrono::{DateTime, Local};
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, Publish, QoS};
use std::borrow::ToOwned;
use std::string::String;
use std::time::Duration;
use std::vec::Vec;

pub struct MqttConfig {
    pub url: String,
    pub port: u16,
    pub keep_alive: Duration,
    pub meshtastic_channel: Option<String>,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            url: "broker.emqx.io".to_owned(),
            port: 1883,
            keep_alive: Duration::from_secs(15),
            meshtastic_channel: None,
        }
    }
}

pub struct MqttReceiver {
    client: AsyncClient,
    topics: Vec<String>,
    event_loop: EventLoop,
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
        let mut mqttoptions = MqttOptions::new("rumqtt-async", config.url, config.port);
        mqttoptions.set_keep_alive(config.keep_alive);

        let (client, event_loop) = AsyncClient::new(mqttoptions, 128);
        let mut topics = Vec::new();
        for mac in macs {
            if mac.is_full() {
                topics.push(std::format!("yar/{mac}/status"));
                topics.push(std::format!("yar/{mac}/p"));
            } else {
                topics.push(std::format!("yar/2/e/serial/!{mac}"));
                if let Some(meshtastic_channel) = config.meshtastic_channel.as_ref() {
                    topics.push(std::format!("yar/2/e/{meshtastic_channel}/!{mac}"));
                }
            }
        }

        Self {
            client,
            event_loop,
            topics,
        }
    }

    fn extract_cell_mac(topic: &str) -> crate::Result<MacAddress> {
        if topic.len() < 16 {
            return Err(Error::ParseError);
        }
        MacAddress::try_from(&topic[4..16])
    }

    fn extract_msh_mac(topic: &str) -> crate::Result<MacAddress> {
        if topic.len() <= 18 {
            // 8 for MAC, 10 for yar/2/e/.../!
            return Err(Error::ParseError);
        }
        MacAddress::try_from(&topic[topic.len() - 8..])
    }

    fn process_incoming(
        now: DateTime<Local>,
        topic: &str,
        payload: &[u8],
    ) -> crate::Result<Message> {
        if let Some(topic) = topic.strip_prefix("yar/2/e/") {
            match topic {
                "serial" => Ok(Message::MeshtasticSerial(now, payload.into())),
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
                _ => Err(Error::ValueError),
            }
        }
    }

    pub async fn next_message(&mut self) -> crate::Result<Message> {
        loop {
            let notification = self.event_loop.poll().await.map_err(|_| Error::ConnectionError)?;
            let now = Local::now();
            match notification {
                Event::Incoming(Packet::Publish(Publish { payload, topic, .. })) => {
                    return Self::process_incoming(now, &topic, &payload);
                }
                Event::Incoming(Packet::Disconnect) => {
                    std::println!("MQTT Disconnected");
                }
                Event::Incoming(Packet::ConnAck(_)) => {
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
        let macs = std::vec![
            MacAddress::Meshtastic(0x12345678),
            MacAddress::Full(0xdeadbeef9876)
        ];
        let config = MqttConfig {
            meshtastic_channel: Some("cha".to_owned()),
            ..Default::default()
        };
        let receiver = MqttReceiver::new(config, macs.iter());
        assert_eq!(
            receiver.topics,
            std::vec![
                "yar/2/e/serial/!12345678",
                "yar/2/e/cha/!12345678",
                "yar/deadbeef9876/status",
                "yar/deadbeef9876/p",
            ]
        );
    }

    #[test]
    fn test_extract_cell_mac() {
        let mac_address = MqttReceiver::extract_cell_mac("yar/deadbeef9876/p").unwrap();
        assert_eq!("deadbeef9876", std::format!("{mac_address}"));
        assert!(MqttReceiver::extract_cell_mac("yar/deadbeef987").is_err());
    }

    #[test]
    fn test_extract_msh_mac() {
        let mac_address = MqttReceiver::extract_msh_mac("yar/2/e/cha/!12345678").unwrap();
        assert_eq!("12345678", std::format!("{mac_address}"));
        assert!(MqttReceiver::extract_cell_mac("yar/2/e/cha/!1234567").is_err());
        assert!(MqttReceiver::extract_cell_mac("yar/2/e//!12345678").is_err());
    }
}
