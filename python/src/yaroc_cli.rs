use std::io::{Read, Write};
use std::time::Duration;

use clap::Parser;
use log::{error, info};
use postcard::to_stdvec;
use pyo3::prelude::*;
use yaroc_common::{
    bg77::modem_manager::ModemConfig,
    usb::{UsbCommand, UsbResponse},
};

use crate::config::{Args, Config};

#[pyfunction]
pub fn yaroc_cli() {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .format_timestamp_millis()
        .try_init();

    let args = Args::parse_from(std::env::args().skip(1));
    let mut serial = tokio_serial::new(&args.port, 112800)
        .timeout(Duration::from_secs(2))
        .open_native()
        .expect("Unable to open serial port");

    let config_str = std::fs::read_to_string(&args.config).expect("Unable to read config file");
    let config: Config = toml::from_str(&config_str).expect("Unable to parse config file");

    let modem_config: ModemConfig = config.modem.into();
    let buf = to_stdvec::<_>(&UsbCommand::ConfigureModem(modem_config)).unwrap();
    if let Err(e) = serial.write_all(buf.as_slice()) {
        error!("Writing to serial failed: {e}");
    } else {
        info!("Writing to serial successful");
        let mut read_buf = [0u8; 64];
        match serial.read(&mut read_buf) {
            Ok(n) => match postcard::from_bytes::<UsbResponse>(&read_buf[..n]) {
                Ok(UsbResponse::Ok) => info!("Configuration successful"),
                Err(e) => error!("Failed to parse response: {e}"),
            },
            Err(e) => error!("Reading from serial failed: {e}"),
        }
    }
}
