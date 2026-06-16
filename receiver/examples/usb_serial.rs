//! An example that monitors and listens to both Meshtastic and SportIdent serial devices.

use clap::Parser;
use log::{error, info};
use yaroc_receiver::{
    message_handler::{MessageHandlerBuilder, SportIdentConfig, UsbSerialConfig},
    state::Event,
    system_info::MacAddress,
};

#[derive(Parser, Debug)]
struct Args {
    #[arg(short, long)]
    dns: Vec<String>,
}

#[tokio::main]
async fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .filter_module("meshtastic", log::LevelFilter::Info)
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

    let usb_serial_config = UsbSerialConfig {
        enable_meshtastic: true,
        sportident: SportIdentConfig::Passive,
    };
    let mut msg_handler = MessageHandlerBuilder::new()
        .with_dns(dns)
        .with_usb_serial_config(usb_serial_config)
        .build();

    info!(
        "Everything initialized, listening for any connected Meshtastic or SportIdent devices..."
    );
    loop {
        tokio::select! {
            event = msg_handler.next_event() => {
                match event {
                    Ok(event) => match event {
                        Event::CellularLog(cellular_log_message) => {
                            info!("Cellular: {cellular_log_message}");
                        }
                        Event::SiPunches(si_punch_logs) => {
                            for punch in si_punch_logs {
                                info!("Punches: {punch}");
                            }
                        }
                        Event::SiPunchesMeshtastic(si_punch_logs, _) => {
                            for punch in si_punch_logs {
                                info!("Meshtastic punches: {punch}");
                            }
                        }
                        Event::SiPunch(punch) => {
                            info!("SI Punch: {punch:?}");
                        }
                        Event::MeshtasticLog(log, _) => {
                            info!("Meshtastic: {log}");
                        }
                        Event::NodeInfos(node_infos) => {
                            info!("Node Infos: {node_infos:?}");
                        }
                        Event::DeviceEvent { added, device } => {
                            info!("Device Event: added={added}, device={device}");
                        }
                    },
                    Err(err) => error!("Error: {err}"),
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl-C received, shutting down...");
                break;
            }
        }
    }
}
