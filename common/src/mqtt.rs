extern crate std;

use crate::error::Error;
use crate::logs::CellularLogMessage;
use crate::proto::Status;
use crate::status::MacAddress;

use chrono::Local;
use femtopb::Message as _;
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, Publish, QoS};
use std::collections::HashMap;
use std::string::{String, ToString};
use std::time::Duration;

struct ClientConfig {
    url: String,
    port: u16,
    keep_alive: Duration,
}

struct MqttClient {
    event_loop: EventLoop,
    dns: HashMap<String, String>,
}

const UNKNOWN: &str = "Unknown";

#[allow(dead_code)]
impl MqttClient {
    pub async fn new(config: ClientConfig, macs: std::vec::Vec<MacAddress>) -> Self {
        let mut mqttoptions = MqttOptions::new("rumqtt-async", config.url, config.port);
        mqttoptions.set_keep_alive(config.keep_alive);

        let (client, event_loop) = AsyncClient::new(mqttoptions, 10);
        for mac in &macs {
            client
                .subscribe(std::format!("yar/{mac}/status"), QoS::AtMostOnce)
                .await
                .unwrap();
        }

        let dns = std::collections::HashMap::new();
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
                _ => {
                    // ignored
                }
            }
        }
    }
}
