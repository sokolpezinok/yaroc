//! High-level typed RPC client for communicating with a YAROC device over USB serial.

use std::io::{Read, Write};

use log::info;
use postcard::to_stdvec;
use yaroc_common::bg77::modem_manager::ModemConfig;
use yaroc_common::send_punch::DeviceConfig;
use yaroc_common::usb::{UsbCommand, UsbResponse};

use crate::config::Config;

use super::transport::{PostcardReader, SetTimeout};

/// Typed client wrapping a USB serial stream for executing YAROC device commands.
pub struct UsbDeviceClient<S> {
    reader: PostcardReader<S>,
}

impl<S: Read + Write + SetTimeout> UsbDeviceClient<S> {
    /// Creates a new [`UsbDeviceClient`] wrapping the given serial stream.
    pub fn new(stream: S) -> Self {
        Self {
            reader: PostcardReader::new(stream),
        }
    }

    /// Sends a [`UsbCommand`] and returns a streaming [`PostcardReader`] for its responses.
    fn send_command_responses(
        &mut self,
        command: &UsbCommand,
    ) -> Result<&mut PostcardReader<S>, String> {
        let buf = to_stdvec(command).map_err(|e| format!("Serialization failed: {e}"))?;
        self.reader
            .get_mut()
            .write_all(buf.as_slice())
            .map_err(|e| format!("Writing to USB serial failed: {e}"))?;

        Ok(&mut self.reader)
    }

    /// Sends a [`UsbCommand`] and waits for a single [`UsbResponse`].
    fn send_command(&mut self, command: &UsbCommand) -> Result<UsbResponse, String> {
        let reader = self.send_command_responses(command)?;
        reader
            .read_next_response()?
            .ok_or_else(|| "Serial port closed before receiving response".to_string())
    }

    /// Sends a command and verifies that the device responds with [`UsbResponse::Ok`].
    fn expect_ok(
        &mut self,
        command: &UsbCommand,
        operation_noun: &str,
        operation_verb: &str,
    ) -> Result<(), String> {
        match self.send_command(command) {
            Ok(UsbResponse::Ok) => Ok(()),
            Ok(r) => Err(format!("Unexpected response from {operation_noun}: {r:?}")),
            Err(e) => Err(format!("Failed to {operation_verb}: {e}")),
        }
    }

    /// Performs the USB handshake, validates the `"YAROC"` magic string, and returns
    /// the device protocol version.
    pub fn handshake(&mut self) -> Result<u32, String> {
        match self.send_command(&UsbCommand::Handshake)? {
            UsbResponse::Handshake(magic, version) => {
                if magic.as_str() != "YAROC" {
                    return Err(format!("Unexpected magic string: {magic}"));
                }
                Ok(version)
            }
            resp => Err(format!("Unexpected response to handshake: {resp:?}")),
        }
    }

    /// Fetches the stored device, modem, and MQTT configurations from flash.
    pub fn export_config(&mut self) -> Result<Config, String> {
        match self.send_command(&UsbCommand::GetConfig) {
            Ok(UsbResponse::Config(device_config, modem_config, mqtt_config)) => Ok(
                Config::from_configs(device_config, modem_config, mqtt_config),
            ),
            Ok(r) => Err(format!("Unexpected response from export-config: {r:?}")),
            Err(e) => Err(format!("Failed to get config from device: {e}")),
        }
    }

    /// Erases the device's internal flash storage.
    pub fn erase_flash(&mut self) -> Result<(), String> {
        self.expect_ok(&UsbCommand::EraseFlash, "flash erase", "erase flash")
    }

    /// Applies modem, optional MQTT, and device settings to the connected device.
    ///
    /// Attempts all configuration steps even if an earlier step fails, returning any
    /// collected error messages.
    pub fn apply_config(&mut self, config: Config) -> Result<(), String> {
        let mut errors = Vec::new();

        let modem_config: ModemConfig = config.modem.into();
        match self.expect_ok(
            &UsbCommand::ConfigureModem(modem_config),
            "modem configuration",
            "configure modem",
        ) {
            Ok(()) => info!("Modem configuration successful"),
            Err(e) => errors.push(e),
        }

        if let Some(mqtt) = config.mqtt {
            match self.expect_ok(
                &UsbCommand::ConfigureMqtt(mqtt.into()),
                "MQTT configuration",
                "configure MQTT",
            ) {
                Ok(()) => info!("MQTT configuration successful"),
                Err(e) => errors.push(e),
            }
        }

        let device_config = DeviceConfig {
            minicallhome_interval: embassy_time::Duration::from_secs(config.minicallhome_interval),
            srr_rx_pin: config.srr_rx_pin.into(),
            ..Default::default()
        };
        match self.expect_ok(
            &UsbCommand::ConfigureDevice(device_config),
            "device configuration",
            "configure device",
        ) {
            Ok(()) => info!("Device configuration successful"),
            Err(e) => errors.push(e),
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }

    /// Requests stored MiniCallHome logs and returns a streaming reader over the responses.
    pub fn stream_mch_logs(&mut self) -> Result<&mut PostcardReader<S>, String> {
        self.send_command_responses(&UsbCommand::GetMiniCallHomeLogs)
    }

    /// Requests stored modem AT response logs and returns a streaming reader over the responses.
    pub fn stream_modem_logs(&mut self) -> Result<&mut PostcardReader<S>, String> {
        self.send_command_responses(&UsbCommand::GetLoggedAtResponseLogs)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use heapless::String as HString;
    use postcard::take_from_bytes;
    use yaroc_common::bg77::modem_manager::RAT;
    use yaroc_common::mqtt::MqttConfig;
    use yaroc_common::send_punch::UartRxPin;

    use crate::config::{LteBandsToml, ModemConfigToml, MqttConfigToml, RatToml, SrrRxPin};

    use super::super::transport::test_utils::MockSerialStream;
    use super::*;

    fn encode_responses(responses: &[UsbResponse]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for resp in responses {
            bytes.extend_from_slice(&to_stdvec(resp).unwrap());
        }
        bytes
    }

    fn decode_written_commands(mut bytes: &[u8]) -> Vec<UsbCommand> {
        let mut commands = Vec::new();
        while !bytes.is_empty() {
            let (cmd, rest) = take_from_bytes::<UsbCommand>(bytes).unwrap();
            commands.push(cmd);
            bytes = rest;
        }
        commands
    }

    #[test]
    fn test_handshake_success() {
        let magic = HString::try_from("YAROC").unwrap();
        let stream = MockSerialStream::new(encode_responses(&[UsbResponse::Handshake(magic, 2)]));
        let mut client = UsbDeviceClient::new(stream);

        let version = client.handshake().unwrap();
        assert_eq!(version, 2);
        assert!(matches!(
            decode_written_commands(&client.reader.get_ref().written).as_slice(),
            [UsbCommand::Handshake]
        ));
    }

    #[test]
    fn test_handshake_invalid_magic() {
        let magic = HString::try_from("OTHER").unwrap();
        let stream = MockSerialStream::new(encode_responses(&[UsbResponse::Handshake(magic, 1)]));
        let mut client = UsbDeviceClient::new(stream);

        let err = client.handshake().unwrap_err();
        assert!(err.contains("Unexpected magic string"));
    }

    #[test]
    fn test_handshake_unexpected_response() {
        let stream = MockSerialStream::new(encode_responses(&[UsbResponse::Ok]));
        let mut client = UsbDeviceClient::new(stream);

        let err = client.handshake().unwrap_err();
        assert!(err.contains("Unexpected response to handshake"));
    }

    #[test]
    fn test_erase_flash_success_with_partial_ok() {
        let stream = MockSerialStream::new(encode_responses(&[
            UsbResponse::PartialOk(150_000),
            UsbResponse::Ok,
        ]));
        let mut client = UsbDeviceClient::new(stream);

        client.erase_flash().unwrap();
        assert!(matches!(
            decode_written_commands(&client.reader.get_ref().written).as_slice(),
            [UsbCommand::EraseFlash]
        ));
        assert_eq!(
            client.reader.get_ref().timeouts,
            vec![Duration::from_millis(150_000)]
        );
    }

    #[test]
    fn test_erase_flash_unexpected_response() {
        let magic = HString::try_from("YAROC").unwrap();
        let stream = MockSerialStream::new(encode_responses(&[UsbResponse::Handshake(magic, 1)]));
        let mut client = UsbDeviceClient::new(stream);

        let err = client.erase_flash().unwrap_err();
        assert!(err.contains("Unexpected response from flash erase"));
    }

    #[test]
    fn test_export_config_success() {
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
        let mqtt = MqttConfig {
            url: HString::try_from("broker.example.com").unwrap(),
            credentials: None,
            packet_timeout: embassy_time::Duration::from_secs(35),
            port: 1883,
        };

        let stream = MockSerialStream::new(encode_responses(&[UsbResponse::Config(
            Some(device),
            Some(modem),
            Some(mqtt),
        )]));
        let mut client = UsbDeviceClient::new(stream);

        let config = client.export_config().unwrap();
        assert_eq!(config.minicallhome_interval, 45);
        assert_eq!(config.srr_rx_pin, SrrRxPin::Sda);
        assert_eq!(config.modem.apn, "internet.iot");
        assert_eq!(config.modem.rat, RatToml::NbIot);
        assert_eq!(config.mqtt.unwrap().url, "broker.example.com");
        assert!(matches!(
            decode_written_commands(&client.reader.get_ref().written).as_slice(),
            [UsbCommand::GetConfig]
        ));
    }

    #[test]
    fn test_export_config_unexpected_response() {
        let stream = MockSerialStream::new(encode_responses(&[UsbResponse::Ok]));
        let mut client = UsbDeviceClient::new(stream);

        let err = client.export_config().unwrap_err();
        assert!(err.contains("Unexpected response from export-config"));
    }

    #[test]
    fn test_apply_config_with_mqtt() {
        let config = Config {
            minicallhome_interval: 60,
            srr_rx_pin: SrrRxPin::Ain1,
            modem: ModemConfigToml {
                apn: "test.apn".to_string(),
                rat: RatToml::Ltem,
                bands: LteBandsToml {
                    ltem: vec![3, 20],
                    nbiot: vec![8],
                },
            },
            mqtt: Some(MqttConfigToml {
                url: "mqtt.example.com".to_string(),
                username: String::new(),
                password: String::new(),
                packet_timeout: 35,
                port: 1883,
            }),
        };

        let stream = MockSerialStream::new(encode_responses(&[
            UsbResponse::Ok,
            UsbResponse::Ok,
            UsbResponse::Ok,
        ]));
        let mut client = UsbDeviceClient::new(stream);

        client.apply_config(config).unwrap();

        let cmds = decode_written_commands(&client.reader.get_ref().written);
        assert_eq!(cmds.len(), 3);
        assert!(matches!(cmds[0], UsbCommand::ConfigureModem(_)));
        assert!(matches!(cmds[1], UsbCommand::ConfigureMqtt(_)));
        assert!(matches!(cmds[2], UsbCommand::ConfigureDevice(_)));
    }

    #[test]
    fn test_apply_config_without_mqtt() {
        let config = Config {
            minicallhome_interval: 30,
            srr_rx_pin: SrrRxPin::Scl,
            modem: ModemConfigToml {
                apn: "test.apn".to_string(),
                rat: RatToml::LtemNbIot,
                bands: LteBandsToml::default(),
            },
            mqtt: None,
        };

        let stream = MockSerialStream::new(encode_responses(&[UsbResponse::Ok, UsbResponse::Ok]));
        let mut client = UsbDeviceClient::new(stream);

        client.apply_config(config).unwrap();

        let cmds = decode_written_commands(&client.reader.get_ref().written);
        assert_eq!(cmds.len(), 2);
        assert!(matches!(cmds[0], UsbCommand::ConfigureModem(_)));
        assert!(matches!(cmds[1], UsbCommand::ConfigureDevice(_)));
    }

    #[test]
    fn test_apply_config_continues_after_error() {
        let config = Config {
            minicallhome_interval: 30,
            srr_rx_pin: SrrRxPin::Scl,
            modem: ModemConfigToml {
                apn: "test.apn".to_string(),
                rat: RatToml::LtemNbIot,
                bands: LteBandsToml::default(),
            },
            mqtt: Some(MqttConfigToml {
                url: "mqtt.example.com".to_string(),
                username: String::new(),
                password: String::new(),
                packet_timeout: 35,
                port: 1883,
            }),
        };

        let magic = HString::try_from("YAROC").unwrap();
        // First command returns an unexpected response, while MQTT and device commands succeed
        let stream = MockSerialStream::new(encode_responses(&[
            UsbResponse::Handshake(magic, 1),
            UsbResponse::Ok,
            UsbResponse::Ok,
        ]));
        let mut client = UsbDeviceClient::new(stream);

        let err = client.apply_config(config).unwrap_err();
        assert!(err.contains("Unexpected response from modem configuration"));

        // Verify all 3 commands were still sent despite the first one failing
        let cmds = decode_written_commands(&client.reader.get_ref().written);
        assert_eq!(cmds.len(), 3);
        assert!(matches!(cmds[0], UsbCommand::ConfigureModem(_)));
        assert!(matches!(cmds[1], UsbCommand::ConfigureMqtt(_)));
        assert!(matches!(cmds[2], UsbCommand::ConfigureDevice(_)));
    }

    #[test]
    fn test_stream_mch_logs() {
        let resp1 = UsbResponse::MiniCallHomeLog(heapless::Vec::from_slice(&[1, 2, 3, 4]).unwrap());
        let resp2 = UsbResponse::MiniCallHomeLog(heapless::Vec::from_slice(&[5, 6, 7, 8]).unwrap());
        let stream = MockSerialStream::new(encode_responses(&[
            UsbResponse::MiniCallHomeLog(heapless::Vec::from_slice(&[1, 2, 3, 4]).unwrap()),
            UsbResponse::MiniCallHomeLog(heapless::Vec::from_slice(&[5, 6, 7, 8]).unwrap()),
            UsbResponse::Ok,
        ]));
        let mut client = UsbDeviceClient::new(stream);

        let logs: Vec<_> =
            client.stream_mch_logs().unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0], resp1);
        assert_eq!(logs[1], resp2);
        assert!(matches!(
            decode_written_commands(&client.reader.get_ref().written).as_slice(),
            [UsbCommand::GetMiniCallHomeLogs]
        ));
    }

    #[test]
    fn test_stream_modem_logs() {
        let resp =
            UsbResponse::LoggedAtResponseLog(heapless::Vec::from_slice(&[10, 20, 30]).unwrap());
        let stream = MockSerialStream::new(encode_responses(&[
            UsbResponse::LoggedAtResponseLog(heapless::Vec::from_slice(&[10, 20, 30]).unwrap()),
            UsbResponse::Ok,
        ]));
        let mut client = UsbDeviceClient::new(stream);

        let logs: Vec<_> =
            client.stream_modem_logs().unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(logs, vec![resp]);
        assert!(matches!(
            decode_written_commands(&client.reader.get_ref().written).as_slice(),
            [UsbCommand::GetLoggedAtResponseLogs]
        ));
    }
}
