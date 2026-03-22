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

#[derive(Debug)]
enum RatToml {
    Ltem,
    NbIot,
    LtemNbIot,
}

impl<'de> Deserialize<'de> for RatToml {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?.to_lowercase().replace("-", "");
        match s.as_str() {
            "ltem" => Ok(RatToml::Ltem),
            "nbiot" => Ok(RatToml::NbIot),
            "both" | "all" | "ltemnbiot" => Ok(RatToml::LtemNbIot),
            _ => Err(serde::de::Error::custom(format!("unknown RAT: {}", s))),
        }
    }
}

impl From<RatToml> for RAT {
    fn from(toml: RatToml) -> Self {
        match toml {
            RatToml::Ltem => RAT::Ltem,
            RatToml::NbIot => RAT::NbIot,
            RatToml::LtemNbIot => RAT::LtemNbIot,
        }
    }
}

#[derive(Deserialize, Debug)]
struct ModemConfigToml {
    apn: String,
    rat: RatToml,
    bands: LteBandsToml,
}

impl From<ModemConfigToml> for ModemConfig {
    fn from(toml: ModemConfigToml) -> Self {
        ModemConfig {
            apn: HString::try_from(toml.apn.as_str()).unwrap_or_default(),
            rat: toml.rat.into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rat_deserialization() {
        #[derive(Deserialize)]
        struct Wrapper {
            rat: RatToml,
        }

        let w: Wrapper = toml::from_str("rat = \"ltem\"").unwrap();
        assert!(matches!(w.rat, RatToml::Ltem));

        let w: Wrapper = toml::from_str("rat = \"NB-IoT\"").unwrap();
        assert!(matches!(w.rat, RatToml::NbIot));

        let w: Wrapper = toml::from_str("rat = \"nbiot\"").unwrap();
        assert!(matches!(w.rat, RatToml::NbIot));

        let w: Wrapper = toml::from_str("rat = \"both\"").unwrap();
        assert!(matches!(w.rat, RatToml::LtemNbIot));

        let w: Wrapper = toml::from_str("rat = \"ALL\"").unwrap();
        assert!(matches!(w.rat, RatToml::LtemNbIot));
    }

    #[test]
    fn test_config_deserialization() {
        let toml_str = r#"
            [modem]
            apn = "test.apn"
            rat = "both"
            [modem.bands]
            ltem = [1, 2, 3]
            nbiot = [20]
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.modem.apn, "test.apn");
        assert!(matches!(config.modem.rat, RatToml::LtemNbIot));
        assert_eq!(config.modem.bands.ltem, vec![1, 2, 3]);
        assert_eq!(config.modem.bands.nbiot, vec![20]);
    }
}
