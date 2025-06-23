extern crate yaroc_common;

use clap::Parser;
use yaroc_common::mqtt::{ClientConfig, MqttClient};

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

    let config = ClientConfig::default();
    let macs = dns.iter().map(|(_, mac)| *mac).collect();
    let mut client = MqttClient::new(config, macs).await;

    loop {
        let msg = client.next_message().await;
        if let Ok(message) = msg {
            println!("{:?}", message);
        }
    }
}
