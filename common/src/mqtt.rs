use crate::flash::{FlashValue, ValueIndex};
use core::str::FromStr;
use embassy_time::Duration;
use heapless::String;
use sequential_storage::map::PostcardValue;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum StatusCode {
    Published,
    Retrying(u8),
    Timeout,
    MqttError,
    Unknown,
}

/// Represents the status of an MQTT message publication.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct MqttStatus {
    pub msg_id: u16,
    pub code: StatusCode,
}

impl MqttStatus {
    /// Creates an `MqttStatus` indicating an MQTT error.
    pub fn mqtt_error(msg_id: u16) -> Self {
        Self {
            msg_id,
            code: StatusCode::MqttError,
        }
    }
}

/// Quality of Service for MQTT messages.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum MqttQos {
    /// At most once.
    Q0 = 0,
    /// At least once.
    Q1 = 1,
    // 2 is unsupported
}

pub mod duration_ms {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(duration.as_millis())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        u64::deserialize(deserializer).map(Duration::from_millis)
    }
}

/// Configuration for the MQTT client to connect to a broker.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct MqttClientConfig {
    /// The URL of the MQTT broker, e.g., "broker.emqx.io".
    pub url: String<50>,
    /// Optional login credentials for the MQTT broker, username and password.
    pub credentials: Option<(String<20>, String<30>)>,
    /// The timeout duration for individual MQTT packets.
    pub packet_timeout: Duration,
    /// The name of the client, used to construct the MQTT client ID.
    pub name: String<24>,
    /// The MAC address of the device, used to form MQTT topics (e.g., "yar/mac_address/topic").
    pub mac_address: String<12>,
    /// The port of the MQTT broker.
    pub port: u16,
}

impl Default for MqttClientConfig {
    fn default() -> Self {
        Self {
            url: String::from_str("broker.emqx.io").unwrap(),
            credentials: None,
            packet_timeout: Duration::from_secs(35),
            name: String::from_str("test_client").unwrap(),
            mac_address: String::from_str("deadbeef").unwrap(),
            port: 1883,
        }
    }
}

impl MqttClientConfig {
    pub fn update(&mut self, reduced_config: MqttConfig) {
        self.url = reduced_config.url;
        self.credentials = reduced_config.credentials;
        self.packet_timeout = reduced_config.packet_timeout;
        self.port = reduced_config.port;
    }
}

/// A reduced version of the MQTT configuration, without fields that are determined in code.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct MqttConfig {
    /// The URL of the MQTT broker, e.g., "broker.emqx.io".
    pub url: String<50>,
    /// Optional login credentials for the MQTT broker, username and password.
    pub credentials: Option<(String<20>, String<30>)>,
    /// The timeout duration for individual MQTT packets.
    #[serde(with = "duration_ms")]
    pub packet_timeout: Duration,
    /// The port of the MQTT broker.
    pub port: u16,
}

impl PostcardValue<'_> for MqttConfig {}

impl FlashValue for MqttConfig {
    const VALUE_INDEX: ValueIndex = ValueIndex::MqttConfig;
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            url: String::from_str("broker.emqx.io").unwrap(),
            credentials: None,
            packet_timeout: Duration::from_secs(35),
            port: 1883,
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::str::FromStr;
    use postcard::{from_bytes, to_slice};

    #[test]
    fn test_mqtt_status_error() {
        let status = MqttStatus::mqtt_error(42);
        assert_eq!(status.msg_id, 42);
        assert_eq!(status.code, StatusCode::MqttError);
    }

    #[test]
    fn test_mqtt_client_config_update() {
        let mut client_config = MqttClientConfig::default();
        let reduced_config = MqttConfig {
            url: String::from_str("mqtt.example.com").unwrap(),
            credentials: Some((
                String::from_str("my_user").unwrap(),
                String::from_str("my_password").unwrap(),
            )),
            packet_timeout: Duration::from_secs(10),
            port: 8883,
        };

        client_config.update(reduced_config.clone());

        assert_eq!(client_config.url, "mqtt.example.com");
        assert_eq!(
            client_config.credentials,
            Some((
                String::from_str("my_user").unwrap(),
                String::from_str("my_password").unwrap()
            ))
        );
        assert_eq!(client_config.packet_timeout, Duration::from_secs(10));
        assert_eq!(client_config.port, 8883);
        // Ensure name and mac address did not change
        assert_eq!(client_config.name, "test_client");
        assert_eq!(client_config.mac_address, "deadbeef");
    }

    #[test]
    fn test_mqtt_config_serialization_deserialization() {
        let config = MqttConfig {
            url: String::from_str("test.mosquitto.org").unwrap(),
            credentials: Some((
                String::from_str("testuser").unwrap(),
                String::from_str("testpass").unwrap(),
            )),
            packet_timeout: Duration::from_secs(60),
            port: 1884,
        };

        let mut buf = [0u8; 256];
        let serialized = to_slice(&config, &mut buf).unwrap();
        let deserialized: MqttConfig = from_bytes(serialized).unwrap();

        assert_eq!(deserialized, config);
    }
}
