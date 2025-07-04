use core::fmt;
use heapless::String;

use crate::error::Error;

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub enum MacAddress {
    Meshtastic(u32),
    Full(u64),
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
            _ => Err(Error::ValueError),
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
    pub name: String<20>,
    pub mac_address: MacAddress,
}

impl HostInfo {
    pub fn new(name: &str, mac_address: MacAddress) -> crate::Result<Self> {
        Ok(Self {
            name: name.try_into().map_err(|_| Error::BufferTooSmallError)?,
            mac_address,
        })
    }
}
