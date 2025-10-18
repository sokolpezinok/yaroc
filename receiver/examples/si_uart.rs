//! An example of reading punches from a SportIdent UART device.

use chrono::Local;
use clap::Parser;
use log::{error, info};

use yaroc_common::{punch::SiPunch, si_uart::SiUart};
use yaroc_receiver::si_uart::TokioSerial;

#[derive(Parser, Debug)]
/// Arguments for the `si_uart` example.
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

    let args = Args::parse();
    let rx = TokioSerial::new(&args.port).unwrap();
    let mut si_uart = SiUart::new(rx);
    info!("Listening for punches on {}", args.port);
    while let Ok(punches) = si_uart.read().await {
        let now = Local::now();
        for punch in punches {
            let punch = SiPunch::from_raw(punch, now.date_naive(), now.offset());
            info!("Recieved punch: {punch:?}");
        }
    }
    error!("Error while receiving punch, exiting");
}
