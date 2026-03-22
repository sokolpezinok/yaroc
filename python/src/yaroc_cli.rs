use std::{io::Write, path::PathBuf};

use clap::Parser;
use heapless::String as HString;
use log::{error, info};
use postcard::to_stdvec;
use pyo3::prelude::*;
use serde::Deserialize;
use yaroc_common::{
    bg77::modem_manager::{LteBands, ModemConfig, RAT},
    usb::UsbCommand,
};

#[derive(Parser, Debug)]
struct Args {
    #[arg(short, long)]
    port: String,
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,
}

#[derive(Deserialize, Debug)]
struct LteBandsToml {
    ltem: Vec<u32>,
    nbiot: Vec<u32>,
}

impl From<LteBandsToml> for LteBands {
    fn from(toml: LteBandsToml) -> Self {
        let mut bands = LteBands::default();
        bands.set_ltem_bands(&toml.ltem);
        bands.set_nbiot_bands(&toml.nbiot);
        bands
    }
}

#[derive(Deserialize, Debug)]
struct ModemConfigToml {
    apn: String,
    rat: RAT,
    bands: LteBandsToml,
}

impl From<ModemConfigToml> for ModemConfig {
    fn from(toml: ModemConfigToml) -> Self {
        ModemConfig {
            apn: HString::try_from(toml.apn.as_str()).unwrap_or_default(),
            rat: toml.rat,
            bands: toml.bands.into(),
        }
    }
}

#[derive(Deserialize, Debug)]
struct Config {
    modem: ModemConfigToml,
}

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
