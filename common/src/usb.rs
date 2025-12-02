use serde::{Deserialize, Serialize};

use crate::bg77::modem_manager::ModemConfig;

#[derive(Serialize, Deserialize)]
pub enum UsbCommand {
    ConfigureModem(ModemConfig),
}

#[derive(Serialize, Deserialize)]
pub enum UsbResponse {
    Ok,
}
