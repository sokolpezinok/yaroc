use crate::error::Error;
use core::fmt;

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
        let mac_address = mac_address.strip_prefix('!').unwrap_or(mac_address);
        match mac_address.len() {
            8 => u32::from_str_radix(mac_address, 16)
                .map(MacAddress::Meshtastic)
                .map_err(|_| Error::ParseError),
            12 => u64::from_str_radix(mac_address, 16)
                .map(MacAddress::Full)
                .map_err(|_| Error::ParseError),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mac_address_try_from() {
        assert_eq!(
            MacAddress::try_from("12345678").unwrap(),
            MacAddress::Meshtastic(0x1234_5678)
        );
        assert_eq!(
            MacAddress::try_from("!12345678").unwrap(),
            MacAddress::Meshtastic(0x1234_5678)
        );
        assert_eq!(
            MacAddress::try_from("1234567890ab").unwrap(),
            MacAddress::Full(0x1234_5678_90ab)
        );
        assert_eq!(
            MacAddress::try_from("!1234567890ab").unwrap(),
            MacAddress::Full(0x1234_5678_90ab)
        );
        assert!(MacAddress::try_from("!1234567").is_err());
        assert!(MacAddress::try_from("1234567").is_err());
    }
}
