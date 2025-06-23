extern crate yaroc_common;

use clap::Parser;
use yaroc_common::mqtt::{MqttConfig, MqttReceiver};

#[derive(Parser, Debug)]
struct Args {
    #[arg(short, long)]
    dns: Vec<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let mut dns = Vec::new();
    for entry in &args.dns {
        if let Some((name, mac)) = entry.split_once(',') {
            dns.push((name, mac));
        }
    }

    let config = MqttConfig::default();
    let macs = dns.iter().map(|(_, mac)| *mac).collect();
    let mut receiver = MqttReceiver::new(config, macs).await;

    loop {
        let msg = receiver.next_message().await;
        if let Ok(message) = msg {
            println!("{:?}", message);
        }
    }
}
