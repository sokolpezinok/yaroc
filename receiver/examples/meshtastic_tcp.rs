use clap::Parser;
use log::{error, info, warn};
use yaroc_receiver::{
    message_handler::MessageHandlerBuilder, state::Event, system_info::MacAddress,
};

#[derive(Parser, Debug)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:4403")]
    host: String,

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

    let mut msg_handler = MessageHandlerBuilder::new().with_dns(dns).with_tcp(args.host).build();

    info!("Everything initialized, listening for Meshtastic TCP events...");
    loop {
        tokio::select! {
            event = msg_handler.next_event() => {
                match event {
                    Ok(event) => match event {
                        Event::SiPunch(punch) => {
                            info!("SI Punch: {punch:?}");
                        }
                        Event::MeshtasticLog(log) => {
                            info!("Meshtastic: {log}");
                        }
                        _ev => {
                            warn!("A non-meshtastic event, this shouldn't happen");
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
