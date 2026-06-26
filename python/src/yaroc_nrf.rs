use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use log::{error, info};
use postcard::to_stdvec;
use pyo3::prelude::*;
use yaroc_common::send_punch::DeviceConfig;
use yaroc_common::{
    bg77::modem_manager::ModemConfig,
    usb::{UsbCommand, UsbResponse},
};

use crate::config::Config;

#[derive(Parser, Debug)]
pub struct Args {
    #[arg(short, long)]
    pub port: String,
    #[arg(short, long, alias = "config", default_value = "nrf52840.toml")]
    pub configure: PathBuf,
    #[arg(long)]
    pub erase_flash: bool,
}

fn send_command<S: Read + Write>(
    serial: &mut S,
    command: UsbCommand,
) -> Result<UsbResponse, String> {
    let buf = to_stdvec(&command).map_err(|e| format!("Serialization failed: {e}"))?;
    serial
        .write_all(buf.as_slice())
        .map_err(|e| format!("Writing to USB serial failed: {e}"))?;

    let mut read_buf = [0u8; 64];
    let n = serial
        .read(&mut read_buf)
        .map_err(|e| format!("Reading from USB serial failed: {e}"))?;
    postcard::from_bytes(&read_buf[..n]).map_err(|e| format!("Failed to parse response: {e}"))
}

#[pyfunction]
pub fn yaroc_nrf() {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .format_timestamp_millis()
        .try_init();

    let args = Args::parse_from(std::env::args().skip(1));
    let mut serial = tokio_serial::new(&args.port, 112800)
        .timeout(Duration::from_secs(2))
        .open_native()
        .expect("Unable to open serial port");

    if args.erase_flash {
        match send_command(&mut serial, UsbCommand::EraseFlash) {
            Ok(UsbResponse::Ok) => info!("Flash erase successful"),
            Err(e) => error!("Failed to erase flash: {e}"),
        }
    }

    let config_path = crate::config::find_config_file(&args.configure);
    match std::fs::read_to_string(&config_path) {
        Ok(config_str) => {
            let config: Config = toml::from_str(&config_str).expect("Unable to parse config file");
            let modem_config: ModemConfig = config.modem.into();
            match send_command(&mut serial, UsbCommand::ConfigureModem(modem_config)) {
                Ok(UsbResponse::Ok) => info!("Modem configuration successful"),
                Err(e) => error!("Failed to configure modem: {e}"),
            }

            if let Some(mqtt) = config.mqtt {
                match send_command(&mut serial, UsbCommand::ConfigureMqtt(mqtt.into())) {
                    Ok(UsbResponse::Ok) => info!("MQTT configuration successful"),
                    Err(e) => error!("Failed to configure MQTT: {e}"),
                }
            }

            let device_config = DeviceConfig {
                minicallhome_interval: embassy_time::Duration::from_secs(
                    config.minicallhome_interval,
                ),
                srr_rx_pin: config.srr_rx_pin.into(),
                ..Default::default()
            };
            match send_command(&mut serial, UsbCommand::ConfigureDevice(device_config)) {
                Ok(UsbResponse::Ok) => info!("Device configuration successful"),
                Err(e) => error!("Failed to configure device: {e}"),
            }
        }
        Err(e) => {
            if args.erase_flash {
                info!("No config file found or readable, skipping configuration: {e}");
            } else {
                panic!("Unable to read config file {}: {e}", config_path.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_parsing() {
        let args = Args::parse_from([
            "test_bin",
            "--port",
            "/dev/ttyACM0",
            "--configure",
            "my_config.toml",
            "--erase-flash",
        ]);
        assert_eq!(args.port, "/dev/ttyACM0");
        assert_eq!(args.configure, PathBuf::from("my_config.toml"));
        assert!(args.erase_flash);

        // Test with config alias
        let args_alias = Args::parse_from([
            "test_bin",
            "--port",
            "/dev/ttyACM0",
            "--config",
            "my_config.toml",
        ]);
        assert_eq!(args_alias.port, "/dev/ttyACM0");
        assert_eq!(args_alias.configure, PathBuf::from("my_config.toml"));
        assert!(!args_alias.erase_flash);
    }
}
