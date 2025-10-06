use chrono::Local;
use clap::Parser;
use log::{error, info};

use yaroc_common::{punch::SiPunch, si_uart::SiUart};
use yaroc_receiver::si_uart::TokioSerial;

#[derive(Parser, Debug)]
struct Args {
    #[arg(short, long)]
    port: String,
}

#[tokio::main]
async fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .format_timestamp_millis()
        .init();

    let rx = TokioSerial::new("/dev/ttyUSB1").unwrap();
    let mut si_uart = SiUart::new(rx);
    while let Ok(punch) = si_uart.read().await {
        let now = Local::now();
        let punch = SiPunch::from_raw(punch, now.date_naive(), now.offset());
        info!("Recieved punch: {punch:?}");
    }
    error!("Error while receiving punch, exiting");
}
