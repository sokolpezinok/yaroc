extern crate yaroc_common;

use clap::Parser;
use log::{error, info};
use yaroc_receiver::message_handler::MessageHandler;
use yaroc_receiver::mqtt::MqttConfig;
use yaroc_receiver::state::Message;
use yaroc_receiver::system_info::MacAddress;

#[derive(Parser, Debug)]
struct Args {
    #[arg(short, long)]
    dns: Vec<String>,
    #[arg(short, long)]
    msh_channel: Option<String>,
}

#[tokio::main]
async fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .filter_module("rumqttc::state", log::LevelFilter::Info)
        //TODO: remove this once the logging problem is fixed
        .filter_module(
            "meshtastic::connections::stream_buffer",
            log::LevelFilter::Off,
        )
        .format_timestamp_millis()
        .init();

    let args = Args::parse();
    let mut dns = Vec::new();
    for entry in &args.dns {
        if let Some((name, mac)) = entry.split_once(',') {
            dns.push((name.to_owned(), MacAddress::try_from(mac).unwrap()));
        }
    }

    let mqtt_config = MqttConfig {
        meshtastic_channel: args.msh_channel,
        ..Default::default()
    };
    let mut handler = MessageHandler::new(dns, Some(mqtt_config));

    info!("Everything initialized, starting the loop");
    loop {
        let log_message = handler.next_message().await;

        match log_message {
            Ok(log) => match log {
                Message::CellularLog(cellular_log_message) => {
                    info!("{cellular_log_message}");
                }
                Message::SiPunches(si_punch_logs) => {
                    for punch in si_punch_logs {
                        info!("{punch}");
                    }
                }
                Message::MeshtasticLog => {
                    info!("Got Meshtastic log, currently unsupported");
                }
            },
            Err(err) => error!("{err}"),
        }
    }
}
