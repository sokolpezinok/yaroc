#[derive(PartialEq, Eq)]
pub enum StatusCode {
    Published,
    Retrying(u8),
    Timeout,
    MqttError,
    Unknown,
}

#[derive(PartialEq, Eq)]
pub struct MqttStatus {
    pub msg_id: u16,
    pub code: StatusCode,
}

impl MqttStatus {
    pub fn from_bg77_qmtpub(msg_id: u16, status: u8, retries: Option<&u8>) -> Self {
        let status = match status {
            0 => StatusCode::Published,
            1 => StatusCode::Retrying(*retries.unwrap_or(&0)),
            2 => StatusCode::Timeout,
            _ => StatusCode::Unknown,
        };
        Self {
            msg_id,
            code: status,
        }
    }

    pub fn mqtt_error(msg_id: u16) -> Self {
        Self {
            msg_id,
            code: StatusCode::MqttError,
        }
    }
}
