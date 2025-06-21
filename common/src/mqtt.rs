extern crate std;

use crate::error::Error;
use crate::logs::CellularLogMessage;
use crate::proto::Status;
use crate::status::MacAddress;

use chrono::Local;
use femtopb::Message as _;
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, Publish, QoS};
use std::borrow::ToOwned;
use std::collections::HashMap;
use std::string::{String, ToString};
use std::time::Duration;

pub struct ClientConfig {
    url: String,
    port: u16,
    keep_alive: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            url: "broker.emqx.io".to_owned(),
            port: 1883,
            keep_alive: Duration::from_secs(15),
        }
    }
}

pub struct MqttClient {
    event_loop: EventLoop,
    dns: HashMap<String, String>,
}

const UNKNOWN: &str = "Unknown";

impl MqttClient {
    pub async fn new(config: ClientConfig, dns: std::vec::Vec<(&str, &str)>) -> Self {
        let mut mqttoptions = MqttOptions::new("rumqtt-async", config.url, config.port);
        mqttoptions.set_keep_alive(config.keep_alive);

        let (client, event_loop) = AsyncClient::new(mqttoptions, 10);
        for mac in dns.iter().map(|(_, mac)| mac) {
            client
                .subscribe(std::format!("yar/{mac}/status"), QoS::AtMostOnce)
                .await
                .unwrap();
        }

        let dns = dns.into_iter().map(|(name, mac)| (mac.to_owned(), name.to_owned())).collect();
        Self { event_loop, dns }
    }

    fn process_incoming(&self, payload: &[u8], topic: &str) -> crate::Result<CellularLogMessage> {
        let mac_address = MacAddress::try_from(&topic[4..16])?;
        let name = self.dns.get(&mac_address.to_string()).map(|s| s.as_str()).unwrap_or(UNKNOWN);

        let status_proto = Status::decode(payload).map_err(|_| Error::ParseError)?;
        CellularLogMessage::from_proto(status_proto, mac_address, name, &Local)
    }

    pub async fn next_message(&mut self) -> crate::Result<CellularLogMessage> {
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
