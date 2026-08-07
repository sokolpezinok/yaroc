use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use chrono::Local;
use clap::{Parser, Subcommand};
use femtopb::Message as _;
use log::{error, info};
use postcard::{from_bytes, take_from_bytes, to_stdvec};
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

    #[arg(long)]
    pub debug: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug, PartialEq)]
pub enum Command {
    /// Export configuration from flash in TOML format
    ExportConfig,

    /// Erase internal flash storage
    #[command(alias = "erase")]
    EraseFlash,

    /// Configure modem and device settings
    Configure {
        #[arg(short, long, default_value = "nrf52840.toml")]
        config: PathBuf,
    },

    /// Dump device logs
    #[command(name = "dump-logs")]
    DumpLogs {
        #[command(subcommand)]
        log_type: LogType,
    },
}

#[derive(Subcommand, Debug, PartialEq)]
pub enum LogType {
    /// Dump MiniCallHome logs
    Mch {
        #[arg(long)]
        gpx: Option<PathBuf>,
    },
    /// Dump modem logs
    #[command(alias = "at-logs")]
    Modem,
}

struct PostcardReader<'a, S> {
    stream: &'a mut S,
    buf: BytesMut,
}

impl<'a, S: Read> PostcardReader<'a, S> {
    fn new(stream: &'a mut S) -> Self {
        Self {
            stream,
            buf: BytesMut::with_capacity(1024),
        }
    }

    /// Read a frame from the connection.
    ///
    /// Returns `None` if EOF is reached.
    fn read_one<T: serde::de::DeserializeOwned>(&mut self) -> Result<Option<T>, String> {
        let mut temp_buf = [0u8; 1024];
        loop {
            if let Some(item) = self.parse_frame() {
                return Ok(Some(item));
            }

            let n = self
                .stream
                .read(&mut temp_buf)
                .map_err(|e| format!("Reading from USB serial failed: {e}"))?;
            if n == 0 {
                if self.buf.is_empty() {
                    return Ok(None);
                } else {
                    return Err("Serial port closed with incomplete response".to_owned());
                }
            }
            self.buf.extend_from_slice(&temp_buf[..n]);
        }
    }

    fn parse_frame<T: serde::de::DeserializeOwned>(&mut self) -> Option<T> {
        loop {
            if self.buf.is_empty() {
                return None;
            }
            match take_from_bytes::<T>(&self.buf) {
                Ok((item, rest)) => {
                    let consumed = self.buf.len() - rest.len();
                    self.buf.advance(consumed);
                    return Some(item);
                }
                Err(postcard::Error::DeserializeUnexpectedEnd) => {
                    return None;
                }
                Err(e) => {
                    error!("Failed to parse response: {e}");
                    self.buf.advance(1);
                }
            }
        }
    }
}

impl<'a, S: SetTimeout> PostcardReader<'a, S> {
    fn set_timeout(&mut self, timeout: Duration) -> Result<(), String> {
        self.stream.set_timeout(timeout)
    }
}

pub trait SetTimeout {
    fn set_timeout(&mut self, timeout: Duration) -> Result<(), String>;
}

impl<T: serialport::SerialPort> SetTimeout for T {
    fn set_timeout(&mut self, timeout: Duration) -> Result<(), String> {
        serialport::SerialPort::set_timeout(self, timeout).map_err(|e| e.to_string())
    }
}

fn send_command<S: Read + Write + SetTimeout>(
    serial: &mut S,
    command: UsbCommand,
) -> Result<UsbResponse, String> {
    let buf = to_stdvec(&command).map_err(|e| format!("Serialization failed: {e}"))?;
    serial
        .write_all(buf.as_slice())
        .map_err(|e| format!("Writing to USB serial failed: {e}"))?;

    let mut reader = PostcardReader::new(serial);
    loop {
        let response = reader
            .read_one()?
            .ok_or_else(|| "Serial port closed before receiving response".to_string())?;
        match response {
            UsbResponse::PartialOk(timeout_ms) => {
                reader.set_timeout(Duration::from_millis(timeout_ms as u64))?;
            }
            resp => return Ok(resp),
        }
    }
}

fn handshake<S: Read + Write + SetTimeout>(serial: &mut S) -> Result<(), String> {
    match send_command(serial, UsbCommand::Handshake)? {
        UsbResponse::Handshake(magic, version) => {
            if magic.as_str() != "YAROC" {
                return Err(format!("Unexpected magic string: {magic}"));
            }
            info!("Connected to YAROC device (protocol v{version})");
            Ok(())
        }
        resp => Err(format!("Unexpected response to handshake: {resp:?}")),
    }
}

fn send_command_multiple_responses<S: Read + Write + SetTimeout>(
    serial: &mut S,
    command: UsbCommand,
) -> Result<Vec<UsbResponse>, String> {
    let buf = to_stdvec(&command).map_err(|e| format!("Serialization failed: {e}"))?;
    serial
        .write_all(buf.as_slice())
        .map_err(|e| format!("Writing to USB serial failed: {e}"))?;

    let mut reader = PostcardReader::new(serial);
    let mut responses = Vec::new();
    info!("Awaiting logs from the device");
    while let Some(response) = reader.read_one()? {
        match response {
            UsbResponse::PartialOk(timeout_ms) => {
                reader.set_timeout(Duration::from_millis(timeout_ms as u64))?;
            }
            UsbResponse::Ok => break,
            other => responses.push(other),
        }
    }
    Ok(responses)
}

fn write_mch_logs_to_csv<W: Write>(
    responses: &[UsbResponse],
    writer: &mut W,
) -> Result<(), String> {
    writeln!(
        writer,
        "timestamp,batt_mv,batt_percents,cpu_temperature,network_type,rsrp_dbm,snr_db,cellid"
    )
    .map_err(|e| format!("Failed to write CSV header: {e}"))?;

    for response in responses {
        if let UsbResponse::MiniCallHomeLog(buf) = response {
            let mch = MiniCallHomeProto::decode(buf.as_slice())
                .map_err(From::from)
                .and_then(MiniCallHome::try_from);
            match mch {
                Ok(mch) => {
                    let timestamp_str = mch
                        .timestamp
                        .map(|t| t.with_timezone(&Local).to_rfc3339())
                        .unwrap_or_default();
                    let batt_mv_str = mch.batt_mv.map(|v| v.to_string()).unwrap_or_default();
                    let batt_percents_str =
                        mch.batt_percents.map(|p| p.to_string()).unwrap_or_default();
                    let cpu_temp_str =
                        mch.cpu_temperature.map(|t| t.to_string()).unwrap_or_default();

                    let (network_type_str, rsrp_dbm_str, snr_db_str, cellid_str) =
                        if let Some(ref signal_info) = mch.signal_info {
                            (
                                format!("{:?}", signal_info.network_type),
                                signal_info.rsrp_dbm.to_string(),
                                format!("{:.1}", signal_info.snr_cb as f32 / 10.0),
                                signal_info
                                    .cellid
                                    .map(|id| format!("{:X}", id))
                                    .unwrap_or_default(),
                            )
                        } else {
                            (String::new(), String::new(), String::new(), String::new())
                        };

                    writeln!(
                        writer,
                        "{},{},{},{},{},{},{},{}",
                        timestamp_str,
                        batt_mv_str,
                        batt_percents_str,
                        cpu_temp_str,
                        network_type_str,
                        rsrp_dbm_str,
                        snr_db_str,
                        cellid_str
                    )
                    .map_err(|e| format!("Failed to write CSV row: {e}"))?;
                }
                Err(e) => {
                    error!("Failed to convert MiniCallHomeProto to MiniCallHome: {e}");
                }
            }
        }
    }
    writer.flush().map_err(|e| format!("Failed to flush CSV writer: {e}"))?;
    Ok(())
}

fn dump_logged_at_response_logs(responses: Vec<UsbResponse>) {
    for response in responses {
        if let UsbResponse::LoggedAtResponseLog(buf) = response {
            match from_bytes::<LoggedAtResponse>(buf.as_slice()) {
                Ok(mut log) => {
                    log.timestamp = log.timestamp.with_timezone(&Local).fixed_offset();
                    info!("{:?}", log);
                }
                Err(e) => {
                    error!("Failed to deserialize LoggedAtResponse: {e}");
                }
            }
        }
    }
}

fn configure<S: Read + Write + SetTimeout>(config: PathBuf, mut serial: S) {
    let config_path = crate::config::find_config_file(&config);
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
            panic!("Unable to read config file {}: {e}", config_path.display());
        }
    }
}

#[pyfunction]
pub fn yaroc_nrf() {
    eprintln!("WARNING: `yaroc-nrf` is deprecated. Please use `yaroc` instead.");
    yaroc_cli();
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
    let mut serial = tokio_serial::new(&args.port, 112800)
        .timeout(Duration::from_millis(1500))
        .open_native()
        .expect("Unable to open serial port");

    if let Err(e) = handshake(&mut serial) {
        error!("USB handshake failed, you probably selected the wrong device: {e}");
        return;
    }

    match args.command {
        Command::ExportConfig => match send_command(&mut serial, UsbCommand::GetConfig) {
            Ok(UsbResponse::Config(device_config, modem_config, mqtt_config)) => {
                let config = Config::from_configs(device_config, modem_config, mqtt_config);
                match toml::to_string_pretty(&config) {
                    Ok(toml_str) => print!("{toml_str}"),
                    Err(e) => error!("Failed to serialize config to TOML: {e}"),
                }
            }
            Ok(r) => error!("Unexpected response from export-config: {r:?}"),
            Err(e) => error!("Failed to get config from device: {e}"),
        },
        Command::EraseFlash => match send_command(&mut serial, UsbCommand::EraseFlash) {
            Ok(UsbResponse::Ok) => info!("Flash erase successful"),
            Ok(r) => error!("Unexpected response from flash erase: {r:?}"),
            Err(e) => error!("Failed to erase flash: {e}"),
        },
        Command::Configure { config } => {
            configure(config, serial);
        }
        Command::DumpLogs { log_type } => match log_type {
            LogType::Mch { gpx } => {
                match send_command_multiple_responses(&mut serial, UsbCommand::GetMiniCallHomeLogs)
                {
                    Ok(responses) => {
                        let mut stdout = std::io::stdout();
                        if let Some(ref gpx_path) = gpx {
                            if let Err(e) = crate::gnss_geotag::geotag_mch_responses(
                                gpx_path,
                                &responses,
                                &mut stdout,
                            ) {
                                error!("Failed to geotag MiniCallHome logs: {e}");
                            }
                        } else if let Err(e) = write_mch_logs_to_csv(&responses, &mut stdout) {
                            error!("Failed to write MiniCallHome logs to stdout: {e}");
                        }
                    }
                    Err(e) => error!("Failed to get MiniCallHome logs: {e}"),
                }
            }
            LogType::Modem => {
                match send_command_multiple_responses(
                    &mut serial,
                    UsbCommand::GetLoggedAtResponseLogs,
                ) {
                    Ok(responses) => dump_logged_at_response_logs(responses),
                    Err(e) => error!("Failed to get LoggedAtResponse logs: {e}"),
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_csv_serialization() {
        use yaroc_common::proto::{CellNetworkType as ProtoCellNetworkType, Timestamp};

        let mch_proto = MiniCallHomeProto {
            freq: 32,
            millivolts: 3600,
            network_type: femtopb::EnumValue::Known(ProtoCellNetworkType::LteM),
            rsrp_dbm: -100,
            signal_snr_cb: 15,
            cellid: 0x12ABCD,
            time: Some(Timestamp {
                millis_epoch: 1782512139000,
                ..Default::default()
            }),
            totaldatarx: 500,
            totaldatatx: 600,
            ..Default::default()
        };

        let mut buf = [0u8; 100];
        let mut slice = buf.as_mut_slice();
        mch_proto.encode(&mut slice).unwrap();
        let encoded_len = mch_proto.encoded_len();

        let response =
            UsbResponse::MiniCallHomeLog(heapless::Vec::from_slice(&buf[..encoded_len]).unwrap());
        let responses = vec![response];

        let mut csv_buf = Vec::new();
        write_mch_logs_to_csv(&responses, &mut csv_buf).unwrap();

        let csv_str = String::from_utf8(csv_buf).unwrap();
        let lines: Vec<&str> = csv_str.trim().split('\n').collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            "timestamp,batt_mv,batt_percents,cpu_temperature,network_type,rsrp_dbm,snr_db,cellid"
        );
        let expected_time = chrono::DateTime::from_timestamp_millis(1782512139000)
            .unwrap()
            .with_timezone(&Local)
            .to_rfc3339();
        assert!(lines[1].contains(&expected_time));
        assert!(lines[1].contains("3600"));
        assert!(lines[1].contains("LteM"));
        assert!(lines[1].contains("-100"));
        assert!(lines[1].contains("1.5"));
        assert!(lines[1].contains("12ABCD"));
    }

    #[test]
    fn test_csv_serialization_edge_cases() {
        // Non-MCH Log variants and corrupted payloads should be skipped
        let responses = vec![
            UsbResponse::Ok,
            // Invalid protobuf payload
            UsbResponse::MiniCallHomeLog(heapless::Vec::from_slice(&[0xFF; 10]).unwrap()),
        ];
        let mut csv_buf = Vec::new();
        write_mch_logs_to_csv(&responses, &mut csv_buf).unwrap();
        let csv_str = String::from_utf8(csv_buf).unwrap();
        assert_eq!(
            csv_str.trim(),
            "timestamp,batt_mv,batt_percents,cpu_temperature,network_type,rsrp_dbm,snr_db,cellid"
        );
    }

    #[test]
    fn test_args_parsing_erase_flash() {
        let args_erase = Args::parse_from(["test_bin", "--port", "/dev/ttyACM0", "erase-flash"]);
        assert_eq!(args_erase.port, "/dev/ttyACM0");
        assert_eq!(args_erase.command, Command::EraseFlash);
    }

    #[test]
    fn test_args_parsing_configure() {
        let args_config = Args::parse_from([
            "test_bin",
            "--port",
            "/dev/ttyACM0",
            "configure",
            "--config",
            "my_config.toml",
        ]);
        assert_eq!(args_config.port, "/dev/ttyACM0");
        assert_eq!(
            args_config.command,
            Command::Configure {
                config: PathBuf::from("my_config.toml")
            }
        );

        let args_config_default =
            Args::parse_from(["test_bin", "--port", "/dev/ttyACM0", "configure"]);
        assert_eq!(
            args_config_default.command,
            Command::Configure {
                config: PathBuf::from("nrf52840.toml")
            }
        );
    }

    #[test]
    fn test_args_parsing_dump_logs() {
        let args_dump_mch = Args::parse_from([
            "test_bin",
            "--port",
            "/dev/ttyACM0",
            "dump-logs",
            "mch",
            "--gpx",
            "track.gpx",
        ]);
        assert_eq!(
            args_dump_mch.command,
            Command::DumpLogs {
                log_type: LogType::Mch {
                    gpx: Some(PathBuf::from("track.gpx"))
                }
            }
        );
    }

    #[test]
    fn test_args_parsing_dump_config() {
        let args_dump_config =
            Args::parse_from(["test_bin", "--port", "/dev/ttyACM0", "export-config"]);
        assert_eq!(args_dump_config.port, "/dev/ttyACM0");
        assert_eq!(args_dump_config.command, Command::ExportConfig);
    }

    struct FragmentedStream {
        data: Vec<u8>,
        read_offset: usize,
        chunk_size: usize,
    }

    impl Read for FragmentedStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.read_offset >= self.data.len() {
                return Ok(0);
            }
            let to_read = self.chunk_size.min(self.data.len() - self.read_offset).min(buf.len());
            buf[..to_read]
                .copy_from_slice(&self.data[self.read_offset..self.read_offset + to_read]);
            self.read_offset += to_read;
            Ok(to_read)
        }
    }

    impl Write for FragmentedStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl SetTimeout for FragmentedStream {
        fn set_timeout(&mut self, _timeout: Duration) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn test_send_command_multiple_responses_fragmented_and_fused() {
        let resp1 = UsbResponse::MiniCallHomeLog(heapless::Vec::from_slice(&[1, 2, 3, 4]).unwrap());
        let resp2 = UsbResponse::MiniCallHomeLog(heapless::Vec::from_slice(&[5, 6, 7, 8]).unwrap());
        let resp_ok = UsbResponse::Ok;

        let mut stream_bytes = Vec::new();
        stream_bytes.extend_from_slice(&to_stdvec(&resp1).unwrap());
        stream_bytes.extend_from_slice(&to_stdvec(&resp2).unwrap());
        stream_bytes.extend_from_slice(&to_stdvec(&resp_ok).unwrap());

        // Test 1: Fragmented reads (1 byte per read)
        let mut frag_stream = FragmentedStream {
            data: stream_bytes.clone(),
            read_offset: 0,
            chunk_size: 1,
        };
        let res =
            send_command_multiple_responses(&mut frag_stream, UsbCommand::GetMiniCallHomeLogs)
                .unwrap();
        assert_eq!(res.len(), 2);
        assert_eq!(res[0], resp1);
        assert_eq!(res[1], resp2);

        // Test 2: Fused reads (all bytes in 1 read)
        let mut fused_stream = FragmentedStream {
            data: stream_bytes,
            read_offset: 0,
            chunk_size: 1024,
        };
        let res =
            send_command_multiple_responses(&mut fused_stream, UsbCommand::GetMiniCallHomeLogs)
                .unwrap();
        assert_eq!(res.len(), 2);
        assert_eq!(res[0], resp1);
        assert_eq!(res[1], resp2);
    }

    #[test]
    fn test_handshake_success() {
        let magic = heapless::String::try_from("YAROC").unwrap();
        let resp = UsbResponse::Handshake(magic, 1);
        let stream_bytes = to_stdvec(&resp).unwrap();
        let mut stream = FragmentedStream {
            data: stream_bytes,
            read_offset: 0,
            chunk_size: 1024,
        };
        assert!(handshake(&mut stream).is_ok());
    }

    #[test]
    fn test_handshake_invalid_magic() {
        let magic = heapless::String::try_from("OTHER").unwrap();
        let resp = UsbResponse::Handshake(magic, 1);
        let stream_bytes = to_stdvec(&resp).unwrap();
        let mut stream = FragmentedStream {
            data: stream_bytes,
            read_offset: 0,
            chunk_size: 1024,
        };
        let err = handshake(&mut stream).unwrap_err();
        assert!(err.contains("Unexpected magic string"));
    }

    #[test]
    fn test_send_command_unexpected_response() {
        let resp = UsbResponse::Ok;
        let stream_bytes = to_stdvec(&resp).unwrap();
        let mut stream = FragmentedStream {
            data: stream_bytes,
            read_offset: 0,
            chunk_size: 1024,
        };
        let err = handshake(&mut stream).unwrap_err();
        assert!(err.contains("Unexpected response to handshake"));
    }

    #[test]
    fn test_send_command_with_partial_ok() {
        let resp_partial = UsbResponse::PartialOk(150_000);
        let resp_ok = UsbResponse::Ok;

        let mut stream_bytes = Vec::new();
        stream_bytes.extend_from_slice(&to_stdvec(&resp_partial).unwrap());
        stream_bytes.extend_from_slice(&to_stdvec(&resp_ok).unwrap());

        let mut stream = FragmentedStream {
            data: stream_bytes,
            read_offset: 0,
            chunk_size: 1024,
        };
        let res = send_command(&mut stream, UsbCommand::EraseFlash).unwrap();
        assert_eq!(res, UsbResponse::Ok);
    }
}
