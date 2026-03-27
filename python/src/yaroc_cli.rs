use std::io::{Read, Write};
use std::time::Duration;

use clap::Parser;
use log::{error, info};
use postcard::to_stdvec;
use pyo3::prelude::*;
use yaroc_common::{
    bg77::modem_manager::ModemConfig,
    bg77::mqtt::MqttConfig,
    usb::{UsbCommand, UsbResponse},
};

use crate::config::{Args, Config};

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
    match send_command(&mut serial, UsbCommand::ConfigureModem(modem_config)) {
        Ok(UsbResponse::Ok) => info!("Modem configuration successful"),
        Err(e) => error!("Failed to configure modem: {e}"),
    }

    if let Some(mqtt) = config.mqtt {
        let mqtt_config: MqttConfig = mqtt.into();
        match send_command(&mut serial, UsbCommand::ConfigureMqtt(mqtt_config)) {
            Ok(UsbResponse::Ok) => info!("MQTT configuration successful"),
            Err(e) => error!("Failed to configure MQTT: {e}"),
        }
    }
}
