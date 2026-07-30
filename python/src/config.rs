use embassy_time::Duration;
use std::path::{Path, PathBuf};
use yaroc_common::send_punch::{DeviceConfig, UartRxPin};

use heapless::String as HString;
use serde::{Deserialize, Serialize};
use yaroc_common::bg77::modem_manager::{LteBands, ModemConfig, RAT};
use yaroc_common::mqtt::MqttConfig;

pub fn find_config_file(path: &Path) -> PathBuf {
    if path.exists() {
        return path.to_path_buf();
    }

    if let Some(file_name) = path.file_name() {
        #[cfg(target_os = "windows")]
        {
            if let Ok(appdata) = std::env::var("APPDATA") {
                let windows_path = Path::new(&appdata).join("yaroc").join(file_name);
                if windows_path.exists() {
                    return windows_path;
                }
            }
            if let Ok(home) = std::env::var("USERPROFILE") {
                let windows_path_fallback =
                    Path::new(&home).join(".config").join("yaroc").join(file_name);
                if windows_path_fallback.exists() {
                    return windows_path_fallback;
                }
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            if let Ok(xdg_config_home) = std::env::var("XDG_CONFIG_HOME") {
                let linux_path = Path::new(&xdg_config_home).join("yaroc").join(file_name);
                if linux_path.exists() {
                    return linux_path;
                }
            } else if let Ok(home) = std::env::var("HOME") {
                let linux_path = Path::new(&home).join(".config").join("yaroc").join(file_name);
                if linux_path.exists() {
                    return linux_path;
                }
            }
        }
    }

    path.to_path_buf()
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq)]
pub struct LteBandsToml {
    pub ltem: Vec<u32>,
    pub nbiot: Vec<u32>,
}

impl Default for LteBandsToml {
    fn default() -> Self {
        Self {
            // Default bands in EU
            ltem: vec![3, 8, 20],
            nbiot: vec![3, 8, 20],
        }
    }
}

impl From<LteBandsToml> for LteBands {
    fn from(toml: LteBandsToml) -> Self {
        let mut bands = LteBands::default();
        bands.set_ltem_bands(&toml.ltem);
        bands.set_nbiot_bands(&toml.nbiot);
        bands
    }
}

impl From<LteBands> for LteBandsToml {
    fn from(bands: LteBands) -> Self {
        let ltem = (1..=128).filter(|&b| (bands.ltem & (1_u128 << (b - 1))) != 0).collect();
        let nbiot = (1..=128).filter(|&b| (bands.nbiot & (1_u128 << (b - 1))) != 0).collect();
        LteBandsToml { ltem, nbiot }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub enum RatToml {
    Ltem,
    NbIot,
    #[default]
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
            _ => Err(serde::de::Error::custom(format!("Unknown RAT: {}", s))),
        }
    }
}

impl Serialize for RatToml {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            RatToml::Ltem => serializer.serialize_str("LTE-M"),
            RatToml::NbIot => serializer.serialize_str("NB-IoT"),
            RatToml::LtemNbIot => serializer.serialize_str("both"),
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

impl From<RAT> for RatToml {
    fn from(rat: RAT) -> Self {
        match rat {
            RAT::Ltem => RatToml::Ltem,
            RAT::NbIot => RatToml::NbIot,
            RAT::LtemNbIot => RatToml::LtemNbIot,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub enum SrrRxPin {
    #[default]
    Scl,
    Sda,
    Ain1,
}

impl<'de> Deserialize<'de> for SrrRxPin {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?.to_lowercase();
        match s.as_str() {
            "scl" => Ok(SrrRxPin::Scl),
            "sda" => Ok(SrrRxPin::Sda),
            "ain1" => Ok(SrrRxPin::Ain1),
            _ => Err(serde::de::Error::custom(format!("Unknown pin: {}", s))),
        }
    }
}

impl Serialize for SrrRxPin {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            SrrRxPin::Scl => serializer.serialize_str("scl"),
            SrrRxPin::Sda => serializer.serialize_str("sda"),
            SrrRxPin::Ain1 => serializer.serialize_str("ain1"),
        }
    }
}

impl From<SrrRxPin> for UartRxPin {
    fn from(value: SrrRxPin) -> Self {
        match value {
            SrrRxPin::Scl => UartRxPin::Scl,
            SrrRxPin::Sda => UartRxPin::Sda,
            SrrRxPin::Ain1 => UartRxPin::Ain1,
        }
    }
}

impl From<UartRxPin> for SrrRxPin {
    fn from(value: UartRxPin) -> Self {
        match value {
            UartRxPin::Scl => SrrRxPin::Scl,
            UartRxPin::Sda => SrrRxPin::Sda,
            UartRxPin::Ain1 => SrrRxPin::Ain1,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq)]
pub struct ModemConfigToml {
    pub apn: String,
    #[serde(default)]
    pub rat: RatToml,
    #[serde(default)]
    pub bands: LteBandsToml,
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

impl From<ModemConfig> for ModemConfigToml {
    fn from(config: ModemConfig) -> Self {
        ModemConfigToml {
            apn: config.apn.to_string(),
            rat: config.rat.into(),
            bands: config.bands.into(),
        }
    }
}

fn default_port() -> u16 {
    1883
}

fn default_packet_timeout() -> u64 {
    35
}

fn default_minicallhome_interval() -> u64 {
    30
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq)]
pub struct MqttConfigToml {
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
    #[serde(default = "default_packet_timeout")]
    pub packet_timeout: u64,
    #[serde(default = "default_port")]
    pub port: u16,
}

impl From<MqttConfigToml> for MqttConfig {
    fn from(toml: MqttConfigToml) -> Self {
        let u = toml.username;
        let p = toml.password;
        let credentials = if u.is_empty() && p.is_empty() {
            None
        } else {
            Some((
                HString::try_from(u.as_str()).unwrap_or_default(),
                HString::try_from(p.as_str()).unwrap_or_default(),
            ))
        };

        MqttConfig {
            url: HString::try_from(toml.url.as_str()).unwrap_or_default(),
            credentials,
            packet_timeout: Duration::from_secs(toml.packet_timeout),
            port: toml.port,
        }
    }
}

impl From<MqttConfig> for MqttConfigToml {
    fn from(config: MqttConfig) -> Self {
        let (username, password) = match config.credentials {
            Some((u, p)) => (u.to_string(), p.to_string()),
            None => (String::new(), String::new()),
        };
        MqttConfigToml {
            url: config.url.to_string(),
            username,
            password,
            packet_timeout: config.packet_timeout.as_secs(),
            port: config.port,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq)]
pub struct Config {
    #[serde(default = "default_minicallhome_interval")]
    pub minicallhome_interval: u64,
    #[serde(default)]
    pub srr_rx_pin: SrrRxPin,
    pub modem: ModemConfigToml,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mqtt: Option<MqttConfigToml>,
}

impl Config {
    pub fn from_configs(
        device_config: Option<DeviceConfig>,
        modem_config: Option<ModemConfig>,
        mqtt_config: Option<MqttConfig>,
    ) -> Self {
        let (minicallhome_interval, srr_rx_pin) = match device_config {
            Some(d) => (d.minicallhome_interval.as_secs(), d.srr_rx_pin.into()),
            None => (default_minicallhome_interval(), SrrRxPin::default()),
        };
        Self {
            minicallhome_interval,
            srr_rx_pin,
            modem: modem_config.unwrap_or_default().into(),
            mqtt: mqtt_config.map(Into::into),
        }
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
            rat = "LTE-M"
            [modem.bands]
            ltem = [1, 2, 3]
            nbiot = [20]
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.modem.apn, "test.apn");
        assert!(matches!(config.modem.rat, RatToml::Ltem));
        assert_eq!(config.modem.bands.ltem, vec![1, 2, 3]);
        assert_eq!(config.modem.bands.nbiot, vec![20]);
        assert_eq!(config.minicallhome_interval, 30); // default value
    }

    #[test]
    fn test_config_deserialization_default_rat() {
        let toml_str = r#"
            [modem]
            apn = "test.apn"
            [modem.bands]
            ltem = [1, 2, 3]
            nbiot = [20]
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.modem.apn, "test.apn");
        assert!(matches!(config.modem.rat, RatToml::LtemNbIot));
    }

    #[test]
    fn test_mqtt_config_deserialization() {
        let toml_str = r#"
            minicallhome_interval = 60
            srr_rx_pin = "sda"

            [modem]
            apn = "test.apn"
            rat = "both"
            [modem.bands]
            ltem = [1, 2, 3]
            nbiot = [20]

            [mqtt]
            url = "mqtt.example.com"
            username = "my_user"
            password = "my_pass"
            packet_timeout = 10
            port = 1883
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.minicallhome_interval, 60);
        assert_eq!(config.srr_rx_pin, SrrRxPin::Sda);

        let mqtt = config.mqtt.unwrap();
        assert_eq!(mqtt.url, "mqtt.example.com");
        assert_eq!(mqtt.username, "my_user".to_string());
        assert_eq!(mqtt.password, "my_pass".to_string());
        assert_eq!(mqtt.packet_timeout, 10);
        assert_eq!(mqtt.port, 1883);

        let mqtt_config: MqttConfig = mqtt.into();
        assert_eq!(
            mqtt_config.credentials,
            Some((
                HString::try_from("my_user").unwrap(),
                HString::try_from("my_pass").unwrap()
            ))
        );
    }

    #[test]
    fn test_mqtt_config_no_credentials() {
        let toml_str_no_creds = r#"
            [modem]
            apn = "test.apn"

            [mqtt]
            url = "mqtt.example.com"
        "#;
        let config_no_creds: Config = toml::from_str(toml_str_no_creds).unwrap();
        assert_eq!(config_no_creds.minicallhome_interval, 30); // default value

        let mqtt_no_creds = config_no_creds.mqtt.unwrap();
        assert_eq!(mqtt_no_creds.username, "");
        assert_eq!(mqtt_no_creds.password, "");
        assert_eq!(mqtt_no_creds.port, 1883);
        assert_eq!(mqtt_no_creds.packet_timeout, 35);

        let mqtt_config_no_creds: MqttConfig = mqtt_no_creds.into();
        assert_eq!(mqtt_config_no_creds.credentials, None);

        // Test with only username specified
        let toml_str_only_username = r#"
            [modem]
            apn = "test.apn"

            [mqtt]
            url = "mqtt.example.com"
            username = "my_user"
            packet_timeout = 5
            port = 1883
        "#;
        let config_only_username: Config = toml::from_str(toml_str_only_username).unwrap();
        let mqtt_only_username = config_only_username.mqtt.unwrap();
        assert_eq!(mqtt_only_username.username, "my_user".to_string());
        assert_eq!(mqtt_only_username.password, "");

        let mqtt_config_only_username: MqttConfig = mqtt_only_username.into();
        assert_eq!(
            mqtt_config_only_username.credentials,
            Some((
                HString::try_from("my_user").unwrap(),
                HString::try_from("").unwrap()
            ))
        );
    }

    #[test]
    fn test_find_config_file() {
        let temp_file_path = std::env::temp_dir().join("test_yaroc_config.toml");
        std::fs::write(&temp_file_path, "").unwrap();
        assert_eq!(find_config_file(&temp_file_path), temp_file_path);
        let _ = std::fs::remove_file(&temp_file_path);

        // Non-existent file
        let non_existent = Path::new("non_existent_config.toml");
        assert_eq!(find_config_file(non_existent), non_existent);

        // Fallback test using XDG_CONFIG_HOME on unix / APPDATA on windows
        #[cfg(not(target_os = "windows"))]
        {
            let config_dir = std::env::temp_dir().join("yaroc_mock_config_unix");
            let yaroc_dir = config_dir.join("yaroc");
            std::fs::create_dir_all(&yaroc_dir).unwrap();
            let mock_config_path = yaroc_dir.join("mock_nrf52840.toml");
            std::fs::write(&mock_config_path, "test").unwrap();

            // Temporarily set XDG_CONFIG_HOME to config_dir
            unsafe {
                std::env::set_var("XDG_CONFIG_HOME", &config_dir);
            }
            let result = find_config_file(Path::new("mock_nrf52840.toml"));
            unsafe {
                std::env::remove_var("XDG_CONFIG_HOME");
            }

            assert_eq!(result, mock_config_path);
            let _ = std::fs::remove_dir_all(&config_dir);
        }

        #[cfg(target_os = "windows")]
        {
            let config_dir = std::env::temp_dir().join("yaroc_mock_config_win");
            let yaroc_dir = config_dir.join("yaroc");
            std::fs::create_dir_all(&yaroc_dir).unwrap();
            let mock_config_path = yaroc_dir.join("mock_nrf52840.toml");
            std::fs::write(&mock_config_path, "test").unwrap();

            // Temporarily set APPDATA to config_dir
            unsafe {
                std::env::set_var("APPDATA", &config_dir);
            }
            let result = find_config_file(Path::new("mock_nrf52840.toml"));
            unsafe {
                std::env::remove_var("APPDATA");
            }

            assert_eq!(result, mock_config_path);
            let _ = std::fs::remove_dir_all(&config_dir);
        }
    }

    #[test]
    fn test_config_serialization() {
        let mut modem = ModemConfig::default();
        modem.apn = HString::try_from("internet.iot").unwrap();
        modem.rat = RAT::NbIot;
        modem.bands.set_ltem_bands(&[3, 8, 20]);
        modem.bands.set_nbiot_bands(&[3, 8, 20]);

        let device = DeviceConfig {
            minicallhome_interval: Duration::from_secs(45),
            srr_rx_pin: UartRxPin::Sda,
            ..Default::default()
        };

        let mqtt = MqttConfig {
            url: HString::try_from("broker.emqx.io").unwrap(),
            credentials: None,
            packet_timeout: Duration::from_secs(35),
            port: 1883,
        };

        let config = Config::from_configs(Some(device), Some(modem), Some(mqtt));
        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(toml_str.contains("minicallhome_interval = 45"));
        assert!(toml_str.contains("srr_rx_pin = \"sda\""));
        assert!(toml_str.contains("apn = \"internet.iot\""));
        assert!(toml_str.contains("rat = \"NB-IoT\""));
        assert!(toml_str.contains("[mqtt]"));
        assert!(toml_str.contains("url = \"broker.emqx.io\""));

        // Roundtrip deserialization
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed, config);
    }
}
