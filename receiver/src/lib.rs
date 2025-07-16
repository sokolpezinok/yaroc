pub mod error;
pub mod logs;
pub mod meshtastic;
pub mod meshtastic_serial;
pub mod message_handler;
pub mod mqtt;
pub mod state;

pub type Result<T> = std::result::Result<T, error::Error>;
