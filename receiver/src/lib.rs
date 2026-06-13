pub mod error;
pub mod logs;
pub mod meshtastic;
pub mod meshtastic_serial;
pub mod meshtastic_tcp;
pub mod message_handler;
pub mod mqtt;
pub mod si_uart;
pub mod state;
pub mod system_info;
pub mod usb_serial_manager;

pub type Result<T> = std::result::Result<T, error::Error>;

#[cfg(test)]
pub mod test_utils;
