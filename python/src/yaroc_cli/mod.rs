//! CLI entrypoints and top-level command execution for interacting with YAROC devices over USB.

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

/// Reads and parses a TOML configuration file and applies it to the connected device.
fn configure<S: Read + Write + SetTimeout>(
    config: &Path,
    client: &mut UsbDeviceClient<S>,
) -> Result<(), String> {
    let config_path = crate::config::find_config_file(config);
    let config_str = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Unable to read config file {}: {e}", config_path.display()))?;
    let parsed_config: Config = toml::from_str(&config_str)
        .map_err(|e| format!("Unable to parse config file {}: {e}", config_path.display()))?;
    client.apply_config(parsed_config)
}

/// Executes a parsed CLI [`Command`] against a connected [`UsbDeviceClient`], writing any
/// command output (such as exported TOML configuration or CSV logs) to `out`.
pub fn execute_command<S, W>(
    client: &mut UsbDeviceClient<S>,
    command: Command,
    out: &mut W,
) -> Result<(), String>
where
    S: Read + Write + SetTimeout,
    W: Write,
{
    match command {
        Command::ExportConfig => {
            let config = client.export_config()?;
            let toml_str = toml::to_string_pretty(&config)
                .map_err(|e| format!("Failed to serialize config to TOML: {e}"))?;
            write!(out, "{toml_str}")
                .map_err(|e| format!("Failed to write exported config: {e}"))?;
            Ok(())
        }
        Command::EraseFlash => {
            client.erase_flash()?;
            info!("Flash erase successful");
            Ok(())
        }
        Command::Configure { config } => configure(&config, client),
        Command::DumpLogs { log_type } => match log_type {
            LogType::Mch { gpx } => {
                let responses = client
                    .stream_mch_logs()
                    .map_err(|e| format!("Failed to get MiniCallHome logs: {e}"))?;
                info!("Awaiting logs from the device");
                if let Some(ref gpx_path) = gpx {
                    let responses = responses
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| format!("Failed to get MiniCallHome logs: {e}"))?;
                    crate::gnss_geotag::geotag_mch_responses(gpx_path, &responses, out)
                        .map_err(|e| format!("Failed to geotag MiniCallHome logs: {e}"))
                } else {
                    write_mch_logs_to_csv(responses, out)
                        .map_err(|e| format!("Failed to write MiniCallHome logs to stdout: {e}"))
                }
            }
            LogType::Modem => {
                let responses = client
                    .stream_modem_logs()
                    .map_err(|e| format!("Failed to get LoggedAtResponse logs: {e}"))?;
                info!("Awaiting logs from the device");
                dump_logged_at_response_logs(responses);
                Ok(())
            }
        },
    }
}

/// Python entrypoint for the `yaroc` CLI tool.
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

    let mut stdout = std::io::stdout();
    if let Err(err) = execute_command(&mut client, args.command, &mut stdout) {
        for line in err.lines() {
            error!("{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use femtopb::Message as _;
    use heapless::String as HString;
    use postcard::to_stdvec;
    use yaroc_common::bg77::modem_manager::{ModemConfig, RAT};
    use yaroc_common::proto::{
        CellNetworkType as ProtoCellNetworkType, MiniCallHome as MiniCallHomeProto, Timestamp,
    };
    use yaroc_common::send_punch::{DeviceConfig, UartRxPin};
    use yaroc_common::usb::UsbResponse;

    use super::logs::MchCsvRow;
    use super::transport::test_utils::MockSerialStream;
    use super::*;

    fn encode_responses(responses: &[UsbResponse]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for resp in responses {
            bytes.extend_from_slice(&to_stdvec(resp).unwrap());
        }
        bytes
    }

    #[test]
    fn test_execute_command_export_config() {
        let device = DeviceConfig {
            minicallhome_interval: embassy_time::Duration::from_secs(45),
            srr_rx_pin: UartRxPin::Sda,
            ..Default::default()
        };
        let modem = ModemConfig {
            apn: HString::try_from("internet.iot").unwrap(),
            rat: RAT::NbIot,
            ..Default::default()
        };
        let stream = MockSerialStream::new(encode_responses(&[UsbResponse::Config(
            Some(device),
            Some(modem),
            None,
        )]));
        let mut client = UsbDeviceClient::new(stream);
        let mut out = Vec::new();

        execute_command(&mut client, Command::ExportConfig, &mut out).unwrap();

        let toml_out = String::from_utf8(out).unwrap();
        assert!(toml_out.contains("minicallhome_interval = 45"));
        assert!(toml_out.contains("srr_rx_pin = \"sda\""));
        assert!(toml_out.contains("apn = \"internet.iot\""));
    }

    #[test]
    fn test_execute_command_erase_flash() {
        let stream = MockSerialStream::new(encode_responses(&[UsbResponse::Ok]));
        let mut client = UsbDeviceClient::new(stream);
        let mut out = Vec::new();

        execute_command(&mut client, Command::EraseFlash, &mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn test_execute_command_configure_from_file() {
        let temp_path = std::env::temp_dir().join("yaroc_cli_test_configure.toml");
        std::fs::write(
            &temp_path,
            r#"
            minicallhome_interval = 40
            srr_rx_pin = "sda"
            [modem]
            apn = "test.apn"
            rat = "LTE-M"
            "#,
        )
        .unwrap();

        let stream = MockSerialStream::new(encode_responses(&[UsbResponse::Ok, UsbResponse::Ok]));
        let mut client = UsbDeviceClient::new(stream);
        let mut out = Vec::new();

        let res = execute_command(
            &mut client,
            Command::Configure {
                config: temp_path.clone(),
            },
            &mut out,
        );
        let _ = std::fs::remove_file(&temp_path);
        res.unwrap();
    }

    #[test]
    fn test_execute_command_dump_logs_mch_csv() {
        let mch_proto = MiniCallHomeProto {
            freq: 32,
            millivolts: 3700,
            network_type: femtopb::EnumValue::Known(ProtoCellNetworkType::LteM),
            rsrp_dbm: -95,
            signal_snr_cb: 20,
            cellid: 0x1234,
            time: Some(Timestamp {
                millis_epoch: 1782512139000,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut buf = [0u8; 100];
        let mut slice = buf.as_mut_slice();
        mch_proto.encode(&mut slice).unwrap();
        let encoded_len = mch_proto.encoded_len();

        let stream = MockSerialStream::new(encode_responses(&[
            UsbResponse::MiniCallHomeLog(heapless::Vec::from_slice(&buf[..encoded_len]).unwrap()),
            UsbResponse::Ok,
        ]));
        let mut client = UsbDeviceClient::new(stream);
        let mut out = Vec::new();

        execute_command(
            &mut client,
            Command::DumpLogs {
                log_type: LogType::Mch { gpx: None },
            },
            &mut out,
        )
        .unwrap();

        let csv_out = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = csv_out.trim().lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], MchCsvRow::HEADER);
        assert!(lines[1].contains("3700"));
    }

    #[test]
    fn test_execute_command_dump_logs_modem() {
        let stream = MockSerialStream::new(encode_responses(&[UsbResponse::Ok]));
        let mut client = UsbDeviceClient::new(stream);
        let mut out = Vec::new();

        execute_command(
            &mut client,
            Command::DumpLogs {
                log_type: LogType::Modem,
            },
            &mut out,
        )
        .unwrap();
        assert!(out.is_empty());
    }
}
