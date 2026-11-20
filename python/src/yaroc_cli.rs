pub mod cli;
pub mod client;
pub mod logs;
pub mod transport;

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use clap::Parser as _;
use log::{error, info};
use pyo3::prelude::*;

pub use cli::{Args, Command, LogType};
pub use client::UsbDeviceClient;
use logs::{dump_logged_at_response_logs, write_mch_logs_to_csv};
pub use transport::SetTimeout;

use crate::config::Config;

fn configure<S: Read + Write + SetTimeout>(config: &Path, client: &mut UsbDeviceClient<S>) {
    let config_path = crate::config::find_config_file(config);
    match std::fs::read_to_string(&config_path) {
        Ok(config_str) => {
            let config: Config = toml::from_str(&config_str).expect("Unable to parse config file");
            if let Err(e) = client.apply_config(config) {
                error!("{e}");
            }
        }
        Err(e) => {
            panic!("Unable to read config file {}: {e}", config_path.display());
        }
    }
}

#[pyfunction]
pub fn yaroc_cli() {
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
    let serial = tokio_serial::new(&args.port, 112800)
        .timeout(Duration::from_millis(1500))
        .open_native()
        .expect("Unable to open serial port");
    let mut client = UsbDeviceClient::new(serial);

    match client.handshake() {
        Ok(version) => info!("Connected to YAROC device (protocol v{version})"),
        Err(e) => {
            error!("USB handshake failed, you probably selected the wrong device: {e}");
            return;
        }
    }

    match args.command {
        Command::ExportConfig => match client.export_config() {
            Ok(config) => match toml::to_string_pretty(&config) {
                Ok(toml_str) => print!("{toml_str}"),
                Err(e) => error!("Failed to serialize config to TOML: {e}"),
            },
            Err(e) => error!("{e}"),
        },
        Command::EraseFlash => match client.erase_flash() {
            Ok(()) => info!("Flash erase successful"),
            Err(e) => error!("{e}"),
        },
        Command::Configure { config } => {
            configure(&config, &mut client);
        }
        Command::DumpLogs { log_type } => match log_type {
            LogType::Mch { gpx } => match client.stream_mch_logs() {
                Ok(responses) => {
                    info!("Awaiting logs from the device");
                    let mut stdout = std::io::stdout();
                    if let Some(ref gpx_path) = gpx {
                        match responses.collect::<Result<Vec<_>, _>>() {
                            Ok(responses) => {
                                if let Err(e) = crate::gnss_geotag::geotag_mch_responses(
                                    gpx_path,
                                    &responses,
                                    &mut stdout,
                                ) {
                                    error!("Failed to geotag MiniCallHome logs: {e}");
                                }
                            }
                            Err(e) => error!("Failed to get MiniCallHome logs: {e}"),
                        }
                    } else if let Err(e) = write_mch_logs_to_csv(responses, &mut stdout) {
                        error!("Failed to write MiniCallHome logs to stdout: {e}");
                    }
                }
                Err(e) => error!("Failed to get MiniCallHome logs: {e}"),
            },
            LogType::Modem => match client.stream_modem_logs() {
                Ok(responses) => {
                    info!("Awaiting logs from the device");
                    dump_logged_at_response_logs(responses);
                }
                Err(e) => error!("Failed to get LoggedAtResponse logs: {e}"),
            },
        },
    }
}
