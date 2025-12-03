use std::io::Write;

use clap::Parser;
use log::{error, info};
use postcard::to_vec;
use pyo3::prelude::*;
use yaroc_common::{bg77::modem_manager::ModemConfig, usb::UsbCommand};

#[derive(Parser, Debug)]
struct Args {
    #[arg(short, long)]
    port: String,
}

#[pyfunction]
pub fn yaroc_cli() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .format_timestamp_millis()
        .init();
    let args = Args::parse_from(std::env::args().skip(1));

    let builder = tokio_serial::new(&args.port, 112800);
    let mut serial = builder.open_native().expect("Unable to open serial port");

    let modem_config = ModemConfig::default();

    let buf = to_vec::<_, 256>(&UsbCommand::ConfigureModem(modem_config)).unwrap();
    if serial.write_all(buf.as_slice()).is_err() {
        error!("Writing to serial failed");
    } else {
        info!("Writing to serial successful");
    }
}
