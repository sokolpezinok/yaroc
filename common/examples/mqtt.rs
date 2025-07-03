extern crate yaroc_common;

use clap::Parser;
use log::{error, info};
use yaroc_common::receive::message_handler::MessageHandler;
use yaroc_common::receive::mqtt::{Message, MqttConfig, MqttReceiver};

#[derive(Parser, Debug)]
struct Args {
    #[arg(short, long)]
    dns: Vec<String>,
}

#[tokio::main]
async fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .filter_module("rumqttc::state", log::LevelFilter::Info)
        .format_timestamp_millis()
        .init();

    let args = Args::parse();
    let mut dns = Vec::new();
    for entry in &args.dns {
        if let Some((name, mac)) = entry.split_once(',') {
            dns.push((mac.to_owned(), name.to_owned()));
        }
    }

    let config = MqttConfig::default();
    let macs = dns.iter().map(|(mac, _)| mac.as_str()).collect();
    let mut receiver = MqttReceiver::new(config, macs);
    let mut handler = MessageHandler::new(dns).unwrap();

    info!("Everything initialized, starting the loop");
    loop {
        let msg = receiver.next_message().await;
        if let Ok(message) = msg {
            match message {
                Message::CellularStatus(mac_address, _, payload) => {
                    let log_message = handler.status_update(&payload, mac_address);
                    match log_message {
                        Ok(log_message) => info!("{log_message}"),
                        Err(err) => error!("{err}"),
                    }
                }
                Message::Punches(mac_address, _, payload) => {
                    let punches = handler.punches(&payload, mac_address);
                    match punches {
                        Ok(punches) => {
                            for punch in &punches {
                                info!("{punch}");
                            }
                        }
                        Err(err) => error!("{err}"),
                    }
                }
            }
        }
    }
}
