use std::path::PathBuf;

use clap::{Parser, Subcommand};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
