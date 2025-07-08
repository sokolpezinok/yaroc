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

    fn process_incoming(
        now: DateTime<Local>,
        topic: &str,
        payload: &[u8],
    ) -> crate::Result<Message> {
        if let Some(topic) = topic.strip_prefix("yar/2/e/") {
            match topic {
                "serial" => Ok(Message::MeshtasticSerial(now, payload.into())),
                _ => {
                    let recv_mac_address = MacAddress::try_from(&topic[topic.len() - 8..])?;
                    Ok(Message::MeshtasticStatus(
                        recv_mac_address,
                        now,
                        payload.into(),
                    ))
                }
            }
        } else {
            let mac_address = MacAddress::try_from(&topic[4..16])?;
            match &topic[16..] {
                "/status" => Ok(Message::CellularStatus(mac_address, now, payload.into())),
                "/p" => Ok(Message::Punches(mac_address, now, payload.into())),
                _ => Err(Error::ValueError),
            }
        }
    }

    pub async fn next_message(&mut self) -> crate::Result<Message> {
        loop {
            let notification = self.event_loop.poll().await.map_err(|_| Error::ParseError)?;
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
