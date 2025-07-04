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
    url: String,
    port: u16,
    keep_alive: Duration,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            url: "broker.emqx.io".to_owned(),
            port: 1883,
            keep_alive: Duration::from_secs(15),
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
}

impl MqttReceiver {
    pub fn new(config: MqttConfig, macs: std::vec::Vec<&str>) -> Self {
        let mut mqttoptions = MqttOptions::new("rumqtt-async", config.url, config.port);
        mqttoptions.set_keep_alive(config.keep_alive);

        let (client, event_loop) = AsyncClient::new(mqttoptions, 10);
        let mut topics = Vec::new();
        for mac in &macs {
            topics.push(std::format!("yar/{mac}/status"));
            topics.push(std::format!("yar/{mac}/p"));
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
        let mac_address = MacAddress::try_from(&topic[4..16])?;
        match &topic[16..] {
            "/status" => Ok(Message::CellularStatus(mac_address, now, payload.into())),
            "/p" => Ok(Message::Punches(mac_address, now, payload.into())),
            _ => Err(Error::ValueError),
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
