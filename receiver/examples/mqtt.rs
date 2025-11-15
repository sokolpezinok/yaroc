extern crate yaroc_common;

use std::time::Duration;

use clap::Parser;
use log::{error, info};
use yaroc_receiver::message_handler::MessageHandler;
use yaroc_receiver::mqtt::MqttConfig;
use yaroc_receiver::state::Event;
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
            dns.push((
                name.to_owned(),
                MacAddress::try_from(mac).expect("MAC address in the wrong format"),
            ));
        } else {
            error!("DNS record in the wrong format, it should be <name>,<MAC_address>");
        }
    }

    let mqtt_config = MqttConfig {
        meshtastic_channel: args.msh_channel.clone(),
        ..Default::default()
    };
    let mqtt_config2 = MqttConfig {
        meshtastic_channel: args.msh_channel,
        url: "broker.hivemq.com".to_owned(),
        ..Default::default()
    };
    let mut handler = MessageHandler::new(
        dns,
        vec![mqtt_config, mqtt_config2],
        Duration::from_secs(60),
    );

    info!("Everything initialized, starting the loop");
    loop {
        let event = handler.next_event().await;
        match event {
            Ok(event) => match event {
                Event::CellularLog(cellular_log_message) => {
                    info!("{cellular_log_message}");
                }
                Event::SiPunches(si_punch_logs) => {
                    for punch in si_punch_logs {
                        info!("{punch}");
                    }
                }
                Event::MeshtasticLog(log) => {
                    info!("{log}");
                }
                Event::NodeInfos(node_infos) => {
                    info!("{node_infos:?}");
                }
            },
            Err(err) => error!("{err}"),
        }
    }
}
