use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use femtopb::Message as _;
use log::{error, info};
use postcard::{from_bytes, to_stdvec};
use pyo3::prelude::*;
use yaroc_common::at::response::LoggedAtResponse;
use yaroc_common::proto::MiniCallHome as MiniCallHomeProto;
use yaroc_common::send_punch::DeviceConfig;
use yaroc_common::status::MiniCallHome;
use yaroc_common::{
    bg77::modem_manager::ModemConfig,
    usb::{UsbCommand, UsbResponse},
};

use crate::config::Config;

#[derive(Parser, Debug)]
pub struct Args {
    #[arg(short, long)]
    pub port: String,
    #[arg(short, long, alias = "config", num_args = 0..=1, default_missing_value = "nrf52840.toml")]
    pub configure: Option<PathBuf>,
    #[arg(long)]
    pub erase_flash: bool,
    #[arg(long)]
    pub dump_mch_logs: bool,
    #[arg(long)]
    pub dump_at_logs: bool,
    #[arg(long)]
    pub debug: bool,
}

fn send_command<S: Read + Write>(
    serial: &mut S,
    command: UsbCommand,
) -> Result<UsbResponse, String> {
    let buf = to_stdvec(&command).map_err(|e| format!("Serialization failed: {e}"))?;
    serial
        .write_all(buf.as_slice())
        .map_err(|e| format!("Writing to USB serial failed: {e}"))?;

    let mut read_buf = [0u8; 1024];
    let n = serial
        .read(&mut read_buf)
        .map_err(|e| format!("Reading from USB serial failed: {e}"))?;
    from_bytes(&read_buf[..n]).map_err(|e| format!("Failed to parse response: {e}"))
}

fn send_command_multiple_responses<S: Read + Write>(
    serial: &mut S,
    command: UsbCommand,
) -> Result<Vec<UsbResponse>, String> {
    let buf = to_stdvec(&command).map_err(|e| format!("Serialization failed: {e}"))?;
    serial
        .write_all(buf.as_slice())
        .map_err(|e| format!("Writing to USB serial failed: {e}"))?;

    let mut read_buf = [0u8; 1024];
    let mut responses = Vec::new();
    info!("Awaiting logs from the device");
    loop {
        let n = serial
            .read(&mut read_buf)
            .map_err(|e| format!("Reading from USB serial failed: {e}"))?;
        match from_bytes(&read_buf[..n]) {
            Ok(UsbResponse::Ok) => break,
            Ok(response) => responses.push(response),
            Err(e) => error!("Failed to parse response: {e}"),
        }
    }
    Ok(responses)
}

fn dump_mini_call_home_logs(responses: Vec<UsbResponse>) {
    for response in responses {
        if let UsbResponse::MiniCallHomeLog(buf) = response {
            let mch = MiniCallHomeProto::decode(buf.as_slice())
                .map_err(From::from)
                .and_then(MiniCallHome::try_from);
            match mch {
                Ok(mch) => {
                    info!("{:?}", mch);
                }
                Err(e) => {
                    error!("Failed to convert MiniCallHomeProto to MiniCallHome: {e}");
                }
            }
        }
    }
}

fn dump_logged_at_response_logs(responses: Vec<UsbResponse>) {
    for response in responses {
        if let UsbResponse::LoggedAtResponseLog(buf) = response {
            match from_bytes::<LoggedAtResponse>(buf.as_slice()) {
                Ok(log) => {
                    info!("{:?}", log);
                }
                Err(e) => {
                    error!("Failed to deserialize LoggedAtResponse: {e}");
                }
            }
        }
    }
}

#[pyfunction]
pub fn yaroc_nrf() {
    let args = Args::parse_from(std::env::args().skip(1));
    let _ = Python::attach(|py| {
        let logging = py.import("logging")?;
        let kwargs = pyo3::types::PyDict::new(py);
        let level = if args.debug { "DEBUG" } else { "INFO" };
        kwargs.set_item("level", logging.getattr(level)?)?;
        // Same as in container.py
        kwargs.set_item(
            "format",
            "%(asctime)s.%(msecs)03d - %(levelname)s - %(message)s",
        )?;
        kwargs.set_item("datefmt", "%H:%M:%S")?;
        logging.call_method("basicConfig", (), Some(&kwargs))?;
        PyResult::Ok(())
    });

    info!("Opening serial port {}", args.port);
    let mut serial = tokio_serial::new(&args.port, 112800)
        .timeout(Duration::from_secs(10))
        .open_native()
        .expect("Unable to open serial port");

    if args.erase_flash {
        match send_command(&mut serial, UsbCommand::EraseFlash) {
            Ok(UsbResponse::Ok) => info!("Flash erase successful"),
            Ok(r) => error!("Unexpected response from flash erase: {r:?}"),
            Err(e) => error!("Failed to erase flash: {e}"),
        }
    }

    if args.dump_mch_logs {
        match send_command_multiple_responses(&mut serial, UsbCommand::GetMiniCallHomeLogs) {
            Ok(responses) => dump_mini_call_home_logs(responses),
            Err(e) => error!("Failed to get MiniCallHome logs: {e}"),
        }
    }

    if args.dump_at_logs {
        match send_command_multiple_responses(&mut serial, UsbCommand::GetLoggedAtResponseLogs) {
            Ok(responses) => dump_logged_at_response_logs(responses),
            Err(e) => error!("Failed to get LoggedAtResponse logs: {e}"),
        }
    }

    let Some(ref configure_path) = args.configure else {
        return;
    };

    let config_path = crate::config::find_config_file(configure_path);
    match std::fs::read_to_string(&config_path) {
        Ok(config_str) => {
            let config: Config = toml::from_str(&config_str).expect("Unable to parse config file");
            let modem_config: ModemConfig = config.modem.into();
            match send_command(&mut serial, UsbCommand::ConfigureModem(modem_config)) {
                Ok(UsbResponse::Ok) => info!("Modem configuration successful"),
                Ok(r) => error!("Unexpected response from modem configuration: {r:?}"),
                Err(e) => error!("Failed to configure modem: {e}"),
            }
            if let Some(mqtt) = config.mqtt {
                match send_command(&mut serial, UsbCommand::ConfigureMqtt(mqtt.into())) {
                    Ok(UsbResponse::Ok) => info!("MQTT configuration successful"),
                    Ok(r) => error!("Unexpected response from MQTT configuration: {r:?}"),
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
                Ok(r) => error!("Unexpected response from device configuration: {r:?}"),
                Err(e) => error!("Failed to configure device: {e}"),
            }
        }
        Err(e) => {
            if args.erase_flash {
                info!("No config file found or not readable, skipping configuration: {e}");
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
            "--dump-mch-logs",
            "--dump-at-logs",
        ]);
        assert_eq!(args.port, "/dev/ttyACM0");
        assert_eq!(args.configure, Some(PathBuf::from("my_config.toml")));
        assert!(args.erase_flash);
        assert!(args.dump_mch_logs);
        assert!(args.dump_at_logs);

        // Test with config alias
        let args_alias = Args::parse_from([
            "test_bin",
            "--port",
            "/dev/ttyACM0",
            "--config",
            "my_config.toml",
        ]);
        assert_eq!(args_alias.port, "/dev/ttyACM0");
        assert_eq!(args_alias.configure, Some(PathBuf::from("my_config.toml")));
        assert!(!args_alias.erase_flash);

        // Test with no configuration specified
        let args_no_config = Args::parse_from(["test_bin", "--port", "/dev/ttyACM0"]);
        assert_eq!(args_no_config.port, "/dev/ttyACM0");
        assert_eq!(args_no_config.configure, None);
        assert!(!args_no_config.erase_flash);

        // Test with configure flag specified but no path
        let args_missing_val =
            Args::parse_from(["test_bin", "--port", "/dev/ttyACM0", "--configure"]);
        assert_eq!(args_missing_val.port, "/dev/ttyACM0");
        assert_eq!(
            args_missing_val.configure,
            Some(PathBuf::from("nrf52840.toml"))
        );
        assert!(!args_missing_val.erase_flash);
        assert!(!args_alias.dump_mch_logs);
    }
}
