extern crate yaroc_common;

use clap::Parser;
use log::{error, info};
use yaroc_common::receive::message_handler::{Message, MessageHandler};
use yaroc_common::receive::mqtt::MqttConfig;

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

    let mqtt_config = MqttConfig::default();
    let mut handler = MessageHandler::new(dns, Some(mqtt_config)).unwrap();

    info!("Everything initialized, starting the loop");
    loop {
        let log_message = handler.next_message().await;

        match log_message {
            Ok(log) => match log {
                Message::CellLog(cellular_log_message) => {
                    info!("{cellular_log_message}");
                }
                Message::SiPunches(si_punch_logs) => {
                    for punch in si_punch_logs {
                        info!("{punch}");
                    }
                }
            },
            Err(err) => error!("{err}"),
        }
    }
}
