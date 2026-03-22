use std::io::Write;

use clap::Parser;
use log::{error, info};
use postcard::to_stdvec;
use pyo3::prelude::*;
use yaroc_common::{bg77::modem_manager::ModemConfig, usb::UsbCommand};

use crate::config::{Args, Config};

#[pyfunction]
pub fn yaroc_cli() {
    let args = Args::parse_from(std::env::args().skip(1));
    let builder = tokio_serial::new(&args.port, 112800);
    let mut serial = builder.open_native().expect("Unable to open serial port");

    let config_str = std::fs::read_to_string(&args.config).expect("Unable to read config file");
    let config: Config = toml::from_str(&config_str).expect("Unable to parse config file");

    let modem_config: ModemConfig = config.modem.into();
    let buf = to_stdvec::<_>(&UsbCommand::ConfigureModem(modem_config)).unwrap();
    if serial.write_all(buf.as_slice()).is_err() {
        error!("Writing to serial failed");
    } else {
        info!("Writing to serial successful");
    }
}
