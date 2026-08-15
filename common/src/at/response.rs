use chrono::{DateTime, FixedOffset};
use core::{fmt::Display, ops::Range, str::FromStr};
#[cfg(feature = "defmt")]
use defmt::error;
use heapless::{String, Vec};
#[cfg(not(feature = "defmt"))]
use log::error;
use serde::{Deserialize, Serialize};

use super::{Error, Result};
use crate::RawMutex;
use crate::status::MiniCallHome;
use embassy_sync::channel::Channel;
use embassy_time::Instant;

// The longest AT command is `AT+QMTCFG="will",...`, at 72 characters
pub const AT_COMMAND_SIZE: usize = 80;
pub const AT_PREFIX_SIZE: usize = 20;
pub const AT_RESPONSE_SIZE: usize = 60;
pub const AT_LINES: usize = 4;
const AT_VALUE_LEN: usize = 40;
const AT_VALUE_COUNT: usize = 8;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
/// Represents a parsed AT command response line.
pub struct CommandResponse {
    line: String<AT_RESPONSE_SIZE>,
    prefix: Range<usize>,
}

impl CommandResponse {
    /// Creates a new `CommandResponse` by parsing a raw AT response line.
    pub fn new(line: &str) -> Result<Self> {
        let (prefix, rest) = Self::split_at_response(line).ok_or(Error::ParseError)?;
        Self::split_values(rest)?; // TODO: store the result to avoid duplicated parsing
        Ok(Self {
            line: String::from_str(line.trim()).map_err(|_| Error::BufferTooSmallError)?,
            prefix: 1..1 + prefix.len(),
        })
    }

    /// Returns the command prefix of the AT response.
    pub fn command(&self) -> &str {
        &self.line[self.prefix.clone()]
    }

    /// Returns a vector of string slices representing the values in the AT response.
    pub fn values(&self) -> Vec<&str, AT_VALUE_COUNT> {
        Self::split_values(&self.line[self.prefix.end + 2..]).unwrap()
    }

    /// Splits an AT response line into its prefix and the rest of the line containing values.
    fn split_at_response(line: &str) -> Option<(&str, &str)> {
        if line.starts_with('+')
            && let Some(prefix_len) = line.find(": ")
        {
            let prefix = &line[1..prefix_len];
            let rest = &line[prefix_len + 2..];
            return Some((prefix, rest));
        }
        None
    }

    /// Parse values from an AT command response.
    ///
    /// Double quotes for strings are ignored. Numbers are returned as strings. For example,
    /// 1,"google.com",15 is parsed into ["1", "google.com", "15"].
    fn split_values(mut values: &str) -> Result<Vec<&str, AT_VALUE_COUNT>> {
        let mut split = Vec::new();
        while !values.is_empty() {
            let pos = match values.chars().next() {
                Some('"') => {
                    let pos = values.find("\",").unwrap_or(values.len() - 1);
                    if pos == values.len() - 1 && !values.ends_with("\"") {
                        // This can happen in the `unwrap_or` branch.
                        return Err(Error::ParseError);
                    }
                    split.push(&values[1..pos]).map_err(|_| Error::BufferTooSmallError)?;
                    pos + 1
                }
                _ => {
                    let pos = values.find(",").unwrap_or(values.len());
                    split.push(&values[..pos]).map_err(|_| Error::BufferTooSmallError)?;
                    pos
                }
            };
            if pos >= values.len() {
                break;
            }
            values = &values[pos + 1..];
        }
        Ok(split)
    }

    /// Pick values from a command response given a list of `indices`.
    fn pick_values<const N: usize>(
        &self,
        indices: [usize; N],
    ) -> Result<Vec<String<AT_VALUE_LEN>, N>> {
        let values = self.values();
        if !indices.iter().all(|idx| *idx < values.len()) {
            return Err(Error::ModemError);
        }
        Ok(indices
            .iter()
            .map(|idx| String::from_str(values[*idx]).unwrap()) //TODO
            .collect())
    }

    /// Parses the values of the command response into a vector of a specified type `T`.
    pub fn parse_values<T: FromStr>(&self) -> Result<Vec<T, AT_VALUE_COUNT>> {
        self.values()
            .iter()
            .map(|val| str::parse::<T>(val).map_err(|_| Error::ParseError))
            .collect::<Result<Vec<_, AT_VALUE_COUNT>>>()
    }
}

impl Display for CommandResponse {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", &self.line)
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for CommandResponse {
    fn format(&self, fmt: defmt::Formatter) {
        // TODO: should we show parsed content?
        defmt::write!(fmt, "{}", self.line)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
/// Represents a raw response from the modem.
pub enum FromModem {
    Ok,
    Error,
    CmeError(u16),
    CommandResponse(CommandResponse),
    Line(String<AT_RESPONSE_SIZE>),
    Eof,
}

impl FromModem {
    /// Returns `true` if the `FromModem` variant indicates a terminal response (Ok, Error, CmeError, Eof).
    #[inline]
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            FromModem::Ok | FromModem::Error | FromModem::CmeError(_) | FromModem::Eof
        )
    }

    /// Returns `true` if the `FromModem` variant indicates an error (Error or CmeError).
    #[inline]
    pub fn is_error(&self) -> bool {
        matches!(self, FromModem::Error | FromModem::CmeError(_))
    }

    pub fn into_error(&self) -> Option<Error> {
        if self == &FromModem::Error {
            Some(Error::AtErrorResponse)
        } else if let FromModem::CmeError(code) = self {
            Some(Error::CmeError(*code))
        } else {
            None
        }
    }
}

impl TryFrom<&str> for FromModem {
    type Error = Error;

    /// Constructs `FromModem` from a line returned by the modem.
    fn try_from(line: &str) -> Result<Self> {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("+CME ERROR:")
            && let Ok(code) = rest.trim().parse::<u16>()
        {
            return Ok(FromModem::CmeError(code));
        }

        match line {
            "OK" | "RDY" | "APP RDY" | ">" => Ok(FromModem::Ok),
            "ERROR" => Ok(FromModem::Error),
            _ => {
                if let Ok(command_response) = CommandResponse::new(line) {
                    Ok(FromModem::CommandResponse(command_response))
                } else {
                    Ok(FromModem::Line(
                        String::from_str(line).map_err(|_| Error::BufferTooSmallError)?,
                    ))
                }
            }
        }
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for FromModem {
    fn format(&self, fmt: defmt::Formatter) {
        match self {
            FromModem::Ok => defmt::write!(fmt, "Ok"),
            FromModem::Error => defmt::write!(fmt, "Error"),
            FromModem::CmeError(code) => defmt::write!(fmt, "CmeError({})", code),
            FromModem::CommandResponse(cmd_response) => {
                defmt::write!(fmt, "{}", cmd_response.line.as_str())
            }
            FromModem::Line(line) => defmt::write!(fmt, "Line({})", line.as_str()),
            FromModem::Eof => defmt::write!(fmt, "Eof"),
        }
    }
}

/// Represents a response from the AT command interface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AtResponse {
    lines: Vec<FromModem, AT_LINES>,
    /// AT command prefix, e.g. `+QMTPUB`, without the initial `AT` and without anything that comes
    /// after.
    command_prefix: String<AT_PREFIX_SIZE>,
}

#[cfg(feature = "defmt")]
impl defmt::Format for AtResponse {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(fmt, "{=[?]}", self.lines.as_slice());
    }
}

impl Display for AtResponse {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self.lines.as_slice())
    }
}

impl AtResponse {
    /// Creates a new `AtResponse` from a vector of `FromModem` lines and the original command.
    pub fn new(lines: Vec<FromModem, AT_LINES>, command: &str) -> Self {
        let pos = command.find(['=', '?']).unwrap_or(command.len());
        let command_prefix = &command[..pos];
        Self {
            lines,
            command_prefix: String::from_str(command_prefix).unwrap(),
        }
    }

    /// Returns `true` if any of the lines in the response are errors.
    pub fn is_error(&self) -> bool {
        self.lines.iter().any(|from_modem| from_modem.is_error())
    }

    /// Returns a slice of the `FromModem` lines contained in this `AtResponse`.
    pub fn lines(&self) -> &[FromModem] {
        self.lines.as_slice()
    }

    /// Forward failed AT response to the flash logger.
    pub fn forward_failed_response(&self) {
        let _ = FLASH_LOG_CHANNEL
            .try_send(FlashLog::AtResponse(PendingLoggedAtResponse {
                response: self.clone(),
                instant: Instant::now(),
            }))
            .inspect_err(|_| error!("Failed to send AT response for logging, channel full"));
    }

    /// Returns a response to the command.
    ///
    /// If `filter` is `None`, it returns the first one.
    /// If `filter` is `(x, idx)`, returns the response with value `x` at position `idx`. If no such
    /// response is found, returns `ModemError`.
    pub fn response<T: FromStr + Eq>(
        &self,
        filter: Option<(T, usize)>,
    ) -> Result<&CommandResponse> {
        for line in &self.lines {
            if let FromModem::CommandResponse(command_response) = line
                && command_response.command() == &self.command_prefix[1..]
            {
                let values = command_response.values();
                match filter.as_ref() {
                    Some((t, idx)) => {
                        let val: Option<T> = values.get(*idx).and_then(|v| str::parse(v).ok());
                        if val.as_ref() == Some(t) {
                            return Ok(command_response);
                        }
                    }
                    None => {
                        return Ok(command_response);
                    }
                }
            }
        }
        self.forward_failed_response();
        Err(Error::ModemError)
    }

    /// Counts the number of values in the first `CommandResponse` that matches the command prefix.
    pub fn count_response_values(&self) -> Result<usize> {
        let response = self.response::<u8>(None)?;
        Ok(response.values().len())
    }

    /// Pick values from an AT response given a list of `indices`.
    ///
    /// If `filter` is `None`, the first AT response is chosen. If `filter` is provided, the response
    /// for which the first chosen value (at position `indices[0]`) matches `filter` is chosen.
    fn pick_values<T: FromStr + Eq, const N: usize>(
        &self,
        indices: [usize; N],
        filter: Option<T>,
    ) -> Result<Vec<String<AT_VALUE_LEN>, N>> {
        self.response(filter.map(|t| (t, indices[0])))?.pick_values(indices)
    }

    /// Parses a string slice into a specified type `T`.
    fn parse<T: FromStr>(s: &str) -> Result<T> {
        str::parse(s).map_err(|_| Error::ParseError)
    }

    /// Parses one value from the AT response into type `T`.
    pub fn parse1<T: FromStr + Eq>(self, indices: [usize; 1]) -> Result<T> {
        let values = self.pick_values::<T, 1>(indices, None)?;
        Self::parse::<T>(&values[0])
    }

    /// Parses two values from the AT response into a tuple `(T, U)`.
    ///
    /// Optionally applies the filter on the first value.
    pub fn parse2<T: FromStr + Eq, U: FromStr>(
        self,
        indices: [usize; 2],
        filter: Option<T>,
    ) -> Result<(T, U)> {
        let values = self.pick_values(indices, filter)?;
        Ok((Self::parse::<T>(&values[0])?, Self::parse::<U>(&values[1])?))
    }

    /// Parses three values from the AT response into a tuple `(T, U, V)`.
    ///
    /// Applies the filter on the first value.
    pub fn parse3<T: FromStr + Eq, U: FromStr, V: FromStr>(
        self,
        indices: [usize; 3],
        filter: T,
    ) -> Result<(T, U, V)> {
        let values = self.pick_values(indices, Some(filter))?;
        Ok((
            Self::parse::<T>(&values[0])?,
            Self::parse::<U>(&values[1])?,
            Self::parse::<V>(&values[2])?,
        ))
    }

    /// Parses four values from the AT response into a tuple `(T, U, V, W)`.
    pub fn parse4<T: FromStr + Eq, U: FromStr, V: FromStr, W: FromStr>(
        self,
        indices: [usize; 4],
    ) -> Result<(T, U, V, W)> {
        let values = self.pick_values::<T, 4>(indices, None)?;
        Ok((
            Self::parse::<T>(&values[0])?,
            Self::parse::<U>(&values[1])?,
            Self::parse::<V>(&values[2])?,
            Self::parse::<W>(&values[3])?,
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
/// Represents an AT response logged with a timestamp.
pub struct LoggedAtResponse {
    pub timestamp: DateTime<FixedOffset>,
    pub response: AtResponse,
}

/// A response from the AT command interface along with the relative time when it was received.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct PendingLoggedAtResponse {
    pub response: AtResponse,
    pub instant: Instant,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[allow(clippy::large_enum_variant)]
pub enum FlashLog {
    AtResponse(PendingLoggedAtResponse),
    MiniCallHome(MiniCallHome),
}

/// A channel for logging messages to flash.
pub static FLASH_LOG_CHANNEL: Channel<RawMutex, FlashLog, 3> = Channel::new();

#[cfg(feature = "defmt")]
impl defmt::Format for LoggedAtResponse {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(
            fmt,
            "LoggedAtResponse {{ timestamp_ms: {}, response: {:?} }}",
            self.timestamp.timestamp_millis(),
            self.response
        );
    }
}

#[cfg(test)]
mod test_at_utils {
    use super::*;

    extern crate std;

    #[test]
    fn test_split_at_response() {
        let res = "+QMTSTAT: 0,2";
        assert_eq!(
            CommandResponse::split_at_response(res),
            Some(("QMTSTAT", "0,2"))
        );

        let res = "QMTSTAT: 0,2";
        assert_eq!(CommandResponse::split_at_response(res), None);
        let res = "+QMTSTAT 0,2";
        assert_eq!(CommandResponse::split_at_response(res), None);
    }

    #[test]
    fn test_cmd_response_split_values() -> Result<()> {
        let ans = CommandResponse::split_values("1,\"item1,item2\",\"cellid\",-7,20")?;
        assert_eq!(&ans, &["1", "item1,item2", "cellid", "-7", "20"]);

        let ans = CommandResponse::split_values("1,\"item1,item2\",\"cellid");
        assert_eq!(ans.unwrap_err(), Error::ParseError);
        Ok(())
    }

    #[test]
    fn test_cmd_response_pick_values() -> Result<()> {
        let response = CommandResponse::new("+CMD: 1,\"item1,item2\",12")?;
        let vals = response.pick_values([1, 2])?;
        assert_eq!(&vals.as_slice(), &["item1,item2", "12"]);
        Ok(())
    }

    #[test]
    fn test_cmd_response_parse_values() -> Result<()> {
        let response = CommandResponse::new("+CMD: 8,13,21")?;
        assert_eq!(response.parse_values::<u8>()?.as_slice(), &[8, 13, 21]);
        Ok(())
    }

    #[test]
    fn test_at_response() -> Result<()> {
        let mut from_modem_vec = Vec::new();
        from_modem_vec.push(FromModem::try_from("+CONN: 1,\"disconnected\"")?).unwrap();
        from_modem_vec.push(FromModem::try_from("+CONN: 5,\"connected\"")?).unwrap();
        let at_response = AtResponse::new(from_modem_vec, "+CONN?");
        let response = at_response.response(Some((5u8, 0)))?;
        assert_eq!(response.values().as_slice(), &["5", "connected"]);

        let response = at_response.response(Some((3u8, 0)));
        assert_eq!(response.unwrap_err(), Error::ModemError);

        let response = at_response.response::<u8>(None)?;
        assert_eq!(response.values().as_slice(), &["1", "disconnected"]);
        Ok(())
    }

    #[test]
    fn test_at_response_parse2() -> crate::Result<()> {
        let from_modem_vec =
            Vec::from_array([FromModem::try_from("+CONN: 1,783,\"disconnected\"")?]);

        let at_response = AtResponse::new(from_modem_vec, "+CONN?");
        assert_eq!(at_response.count_response_values().unwrap(), 3);
        let (id, status) = at_response.parse2::<u8, String<20>>([0, 2], None).unwrap();
        assert_eq!(id, 1);
        assert_eq!(status, "disconnected");

        Ok(())
    }

    #[test]
    fn test_at_response_parse4() -> crate::Result<()> {
        let from_modem_vec =
            Vec::from_array([FromModem::try_from("+QCSQ: \"NBIoT\",0,-131,55,-20")?]);

        let at_response = AtResponse::new(from_modem_vec, "+QCSQ");
        let (rssi_dbm, rsrp_dbm, snr_mult, rsrq_dbm) =
            at_response.parse4::<i8, i16, u8, i8>([1, 2, 3, 4]).unwrap();
        assert_eq!(rssi_dbm, 0);
        assert_eq!(rsrp_dbm, -131);
        assert_eq!(snr_mult, 55);
        assert_eq!(rsrq_dbm, -20);

        Ok(())
    }

    #[test]
    fn test_postcard_serialization() -> crate::Result<()> {
        let from_modem_vec = Vec::from_array([
            FromModem::try_from("+QCSQ: \"NBIoT\",0,-131,55,-20")?,
            FromModem::Ok,
        ]);
        let at_response = AtResponse::new(from_modem_vec, "+QCSQ");

        let mut buf = [0u8; 256];
        let serialized = postcard::to_slice(&at_response, &mut buf).unwrap();
        let deserialized: AtResponse = postcard::from_bytes(serialized).unwrap();

        assert_eq!(at_response, deserialized);
        Ok(())
    }

    #[test]
    fn test_logged_at_response_serialization() -> crate::Result<()> {
        let from_modem_vec = Vec::from_array([
            FromModem::try_from("+QCSQ: \"NBIoT\",0,-131,55,-20")?,
            FromModem::Ok,
        ]);
        let at_response = AtResponse::new(from_modem_vec, "+QCSQ");
        let timestamp = DateTime::parse_from_rfc3339("2023-11-23T10:00:03.793+01:00").unwrap();
        let logged_response = LoggedAtResponse {
            timestamp,
            response: at_response,
        };

        let mut buf = [0u8; 512];
        let serialized = postcard::to_slice(&logged_response, &mut buf).unwrap();
        let deserialized: LoggedAtResponse = postcard::from_bytes(serialized).unwrap();

        assert_eq!(logged_response, deserialized);
        Ok(())
    }

    #[test]
    fn test_max_logged_at_response_serialization() -> crate::Result<()> {
        let line = std::format!("+{:A<10}: {:B<47}", "", "");
        assert_eq!(line.len(), AT_RESPONSE_SIZE);
        let cmd_resp = FromModem::try_from(&line[..])?;
        assert!(matches!(cmd_resp, FromModem::CommandResponse(_)));

        let from_modem_vec = Vec::from_array([
            cmd_resp.clone(),
            cmd_resp.clone(),
            cmd_resp.clone(),
            cmd_resp,
        ]);
        let max_prefix = "B".repeat(AT_PREFIX_SIZE);
        let at_response = AtResponse::new(from_modem_vec, &max_prefix);
        let timestamp =
            DateTime::parse_from_rfc3339("2026-07-16T22:46:15.999999999+02:00").unwrap();
        let logged_response = LoggedAtResponse {
            timestamp,
            response: at_response,
        };

        let mut buf = [0u8; 1024];
        let serialized = postcard::to_slice(&logged_response, &mut buf).unwrap();
        assert!(
            serialized.len() <= 384,
            "Serialized size exceeds 384: {}",
            serialized.len()
        );
        assert!(
            serialized.len() >= 300,
            "Serialized size is unexpectedly small: {}",
            serialized.len()
        );
        Ok(())
    }

    #[test]
    fn test_try_from() -> crate::Result<()> {
        assert_eq!(FromModem::try_from("OK")?, FromModem::Ok);
        assert_eq!(FromModem::try_from("  RDY \r\n")?, FromModem::Ok);
        assert_eq!(FromModem::try_from("APP RDY")?, FromModem::Ok);
        assert_eq!(FromModem::try_from(">")?, FromModem::Ok);
        assert_eq!(FromModem::try_from("ERROR")?, FromModem::Error);
        assert_eq!(
            FromModem::try_from("+CME ERROR: 30")?,
            FromModem::CmeError(30)
        );
        assert!(FromModem::CmeError(30).terminal());
        assert_eq!(
            FromModem::try_from("+QCSQ: \"NBIoT\",0,-131,55,-20")?,
            FromModem::CommandResponse(CommandResponse::new("+QCSQ: \"NBIoT\",0,-131,55,-20")?)
        );
        assert_eq!(FromModem::Error.into_error(), Some(Error::AtErrorResponse));
        assert_eq!(
            FromModem::CmeError(30).into_error(),
            Some(Error::CmeError(30))
        );
        assert_eq!(FromModem::Ok.into_error(), None);
        assert_eq!(
            FromModem::try_from("some raw line")?,
            FromModem::Line(String::from_str("some raw line").unwrap())
        );
        Ok(())
    }
}
