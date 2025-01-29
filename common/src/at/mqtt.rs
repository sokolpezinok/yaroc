#[derive(PartialEq, Eq)]
pub enum MqttPubStatus {
    Published,
    Retrying(u8),
    Timeout,
    MqttError,
    Unknown,
}

#[derive(PartialEq, Eq)]
pub struct MqttPublishReport {
    pub msg_id: u8,
    pub status: MqttPubStatus,
}

impl MqttPublishReport {
    pub fn from_bg77_qmtpub(msg_id: u8, status: u8, retries: Option<&u8>) -> Self {
        let status = match status {
            0 => MqttPubStatus::Published,
            1 => MqttPubStatus::Retrying(*retries.unwrap_or(&0)),
            2 => MqttPubStatus::Timeout,
            _ => MqttPubStatus::Unknown,
        };
        Self { msg_id, status }
    }

    pub fn mqtt_error(msg_id: u8) -> Self {
        Self {
            msg_id,
            status: MqttPubStatus::MqttError,
        }
    }
}
