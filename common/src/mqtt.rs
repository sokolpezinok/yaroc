extern crate std;

use crate::error::Error;
use crate::status::MacAddress;

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
    event_loop: EventLoop,
}

#[derive(Debug)]
pub enum Message {
    CellularStatus(MacAddress, Vec<u8>),
}

impl MqttReceiver {
    pub async fn new(config: MqttConfig, macs: std::vec::Vec<&str>) -> Self {
        let mut mqttoptions = MqttOptions::new("rumqtt-async", config.url, config.port);
        mqttoptions.set_keep_alive(config.keep_alive);

        let (client, event_loop) = AsyncClient::new(mqttoptions, 10);
        for mac in &macs {
            client
                .subscribe(std::format!("yar/{mac}/status"), QoS::AtMostOnce)
                .await
                .unwrap();
        }

        Self { event_loop }
    }

    fn process_incoming(&self, payload: &[u8], topic: &str) -> crate::Result<Message> {
        let mac_address = MacAddress::try_from(&topic[4..16])?;
        Ok(Message::CellularStatus(mac_address, payload.into()))
    }

    pub async fn next_message(&mut self) -> crate::Result<Message> {
        loop {
            let notification = self.event_loop.poll().await.map_err(|_| Error::ParseError)?;
            match notification {
                Event::Incoming(Packet::Publish(Publish { payload, topic, .. })) => {
                    return self.process_incoming(&payload, &topic);
                }
                Event::Incoming(Packet::Disconnect) => {
                    std::println!("Disconnected");
                }
                _ => {
                    // ignored
                }
            }
        }
    }
}
