use clap::Parser;
use log::{error, info};
use std::time::Duration;
use yaroc_receiver::{message_handler::MessageHandler, state::Event, system_info::MacAddress};

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

    let mut msg_handler = MessageHandler::new(dns, Vec::new(), Duration::from_secs(60));
    let mut serial_device_manager = msg_handler.usb_serial_manager(true);

    let monitor_task = tokio::spawn(async move {
        if let Err(e) = serial_device_manager.monitor_usb_devices().await {
            error!("Error in USB monitoring: {e}");
        }
    });

    info!("Everything initialized, starting the loop");
    loop {
        tokio::select! {
            event = msg_handler.next_event() => {
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
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl-C received, shutting down...");
                monitor_task.abort();
                break;
            }
        }
    }
}
