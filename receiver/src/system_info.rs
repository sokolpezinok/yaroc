use crate::error::Error;
use core::fmt;
use std::borrow::ToOwned;

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub enum MacAddress {
    Meshtastic(u32),
    Full(u64),
}

impl MacAddress {
    pub fn is_meshtastic(&self) -> bool {
        matches!(self, MacAddress::Meshtastic(_))
    }

    pub fn is_full(&self) -> bool {
        matches!(self, MacAddress::Full(_))
    }
}

impl TryFrom<&str> for MacAddress {
    type Error = Error;

    fn try_from(mac_address: &str) -> crate::Result<Self> {
        match mac_address.len() {
            8 => Ok(MacAddress::Meshtastic(
                u32::from_str_radix(mac_address, 16).map_err(|_| Error::ParseError)?,
            )),
            12 => Ok(MacAddress::Full(
                u64::from_str_radix(mac_address, 16).map_err(|_| Error::ParseError)?,
            )),
            _ => Err(Error::ParseError),
        }
    }
}

impl Default for MacAddress {
    fn default() -> Self {
        Self::Full(0x1234)
    }
}

impl core::fmt::Display for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MacAddress::Meshtastic(mac) => write!(f, "{:08x}", mac),
            MacAddress::Full(mac) => write!(f, "{:012x}", mac),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostInfo {
    pub name: String,
    pub mac_address: MacAddress,
}

impl HostInfo {
    pub fn new(name: &str, mac_address: MacAddress) -> Self {
        Self {
            name: name.to_owned(),
            mac_address,
        }
    }
}
