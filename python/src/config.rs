use embassy_time::Duration;
use std::path::{Path, PathBuf};

use clap::Parser;
use heapless::String as HString;
use serde::Deserialize;
use yaroc_common::bg77::modem_manager::{LteBands, ModemConfig, RAT};
use yaroc_common::mqtt::MqttConfig;

#[derive(Parser, Debug)]
pub struct Args {
    #[arg(short, long)]
    pub port: String,
    #[arg(short, long, default_value = "nrf52840.toml")]
    pub config: PathBuf,
}

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

#[derive(Deserialize, Debug)]
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

#[derive(Debug, Default)]
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

fn default_port() -> u16 {
    1883
}

fn default_packet_timeout() -> u64 {
    35
}

#[derive(Deserialize, Debug)]
pub struct MqttConfigToml {
    pub url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_packet_timeout")]
    pub packet_timeout: u64,
    pub minicallhome_interval: u64,
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
            minicallhome_interval: Duration::from_secs(toml.minicallhome_interval),
            port: toml.port,
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct Config {
    pub modem: ModemConfigToml,
    pub mqtt: Option<MqttConfigToml>,
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
    }

    #[test]
    fn test_mqtt_config_deserialization() {
        let toml_str = r#"
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
            minicallhome_interval = 60
            port = 1883
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let mqtt = config.mqtt.unwrap();
        assert_eq!(mqtt.url, "mqtt.example.com");
        assert_eq!(mqtt.username, "my_user".to_string());
        assert_eq!(mqtt.password, "my_pass".to_string());
        assert_eq!(mqtt.packet_timeout, 10);
        assert_eq!(mqtt.minicallhome_interval, 60);
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
            minicallhome_interval = 30
        "#;
        let config_no_creds: Config = toml::from_str(toml_str_no_creds).unwrap();
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
            minicallhome_interval = 30
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
}
