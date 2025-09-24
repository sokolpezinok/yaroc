use core::{fmt::Display, str::FromStr};
use heapless::{String, Vec};

use crate::error::Error;

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Substring {
    start: usize,
    end: usize,
}

impl Substring {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn end(&self) -> usize {
        self.end
    }
}

impl Display for Substring {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "({},{})", self.start, self.end)
    }
}

pub const AT_COMMAND_SIZE: usize = 90;
pub const AT_RESPONSE_SIZE: usize = 50;
pub const AT_LINES: usize = 4;
const AT_VALUE_LEN: usize = 40;
const AT_VALUE_COUNT: usize = 8;

#[derive(Clone, Debug, PartialEq)]
pub struct CommandResponse {
    line: String<AT_COMMAND_SIZE>,
    prefix: Substring,
}

impl CommandResponse {
    pub fn new(line: &str) -> crate::Result<Self> {
        let (prefix, rest) = Self::split_at_response(line).ok_or(Error::ParseError)?;
        Self::split_values(rest)?; // TODO: store the result
        Ok(Self {
            line: String::from_str(line).map_err(|_| Error::BufferTooSmallError)?,
            prefix: Substring::new(1, 1 + prefix.len()),
        })
    }

    pub fn command(&self) -> &str {
        &self.line[1..self.prefix.end()]
    }

    pub fn values(&self) -> Vec<&str, AT_VALUE_COUNT> {
        Self::split_values(&self.line[self.prefix.end() + 2..]).unwrap()
    }

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

    /// Parse out values out of a AT command response.
    ///
    /// Double quotes for strings are ignored. Numbers are returned as strings. For example,
    /// 1,"google.com",15 is parsed into ["1", "google.com", "15"].
    fn split_values(mut values: &str) -> Result<Vec<&str, AT_VALUE_COUNT>, Error> {
        let mut split = Vec::new();
        while !values.is_empty() {
            let pos = match values.chars().next() {
                Some('"') => {
                    let pos = values.find("\",").unwrap_or(values.len() - 1);
                    if pos == values.len() - 1 && !values.ends_with("\"") {
                        // This can happen in the `unwrap_or` branch.
                        return Err(Error::ParseError);
                    }
                    split.push(&values[1..pos]).unwrap();
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

    /// Pick values from a command response given by the list of `indices`.
    fn pick_values<const N: usize>(
        &self,
        indices: [usize; N],
    ) -> Result<Vec<String<AT_VALUE_LEN>, N>, Error> {
        let values = self.values();
        if !indices.iter().all(|idx| *idx < values.len()) {
            return Err(Error::ModemError);
        }
        Ok(indices
            .iter()
            .map(|idx| String::from_str(values[*idx]).unwrap()) //TODO
            .collect())
    }

    pub fn parse_values<T: FromStr>(&self) -> Result<Vec<T, AT_VALUE_COUNT>, Error> {
        self.values()
            .iter()
            .map(|val| str::parse::<T>(val).map_err(|_| Error::ParseError))
            .collect::<Result<Vec<_, AT_VALUE_COUNT>, _>>()
    }
}

impl Display for CommandResponse {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.line.trim())
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for CommandResponse {
    fn format(&self, fmt: defmt::Formatter) {
        // TODO: should we show parsed content?
        defmt::write!(fmt, "{}", self.line)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FromModem {
    Line(String<AT_COMMAND_SIZE>),
    CommandResponse(CommandResponse),
    Ok,
    Error,
    Eof,
}

impl FromModem {
    pub fn terminal(&self) -> bool {
        matches!(self, FromModem::Ok | FromModem::Error | FromModem::Eof)
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for FromModem {
    fn format(&self, fmt: defmt::Formatter) {
        match self {
            FromModem::Line(line) => defmt::write!(fmt, "{}", line),
            FromModem::CommandResponse(command_response) => {
                defmt::write!(fmt, "{}", command_response)
            }
            FromModem::Ok => defmt::write!(fmt, "Ok"),
            FromModem::Error => defmt::write!(fmt, "Error"),
            FromModem::Eof => defmt::write!(fmt, "Eof"),
        }
    }
}

pub struct AtResponse {
    lines: Vec<FromModem, AT_LINES>,
    command: String<AT_COMMAND_SIZE>,
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
    pub fn new(lines: Vec<FromModem, AT_LINES>, command: &str) -> Self {
        let pos = command.find(['=', '?']).unwrap_or(command.len());
        let command_prefix = &command[..pos];
        Self {
            lines,
            command: String::from_str(command_prefix).unwrap(),
        }
    }

    pub fn lines(&self) -> &[FromModem] {
        self.lines.as_slice()
    }

    /// Returns a response to the command.
    ///
    /// If `filter` is None, it returns the first one.
    /// If `filter` is `(x, idx)`, returns the response with value `x` on position `idx`. If there
    /// is no such value, returns `ModemError`.
    fn response<T: FromStr + Eq>(
        &self,
        filter: Option<(T, usize)>,
    ) -> Result<&CommandResponse, Error> {
        for line in &self.lines {
            if let FromModem::CommandResponse(command_response) = line
                && command_response.command() == &self.command[1..]
            {
                let values = command_response.values();
                match filter.as_ref() {
                    Some((t, idx)) => {
                        let val: Option<T> = str::parse(values[*idx]).ok();
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
        Err(Error::ModemError)
    }

    pub fn count_response_values(&self) -> Result<usize, Error> {
        let response = self.response::<u8>(None)?;
        Ok(response.values().len())
    }

    /// Pick values from an AT response given by the list of `indices`.
    ///
    /// If filter is None, the first at response is chosen. If `filter` is provided, only the response
    /// for which the first chosen value (at position `indices[0]`) matches `filter`.
    fn pick_values<T: FromStr + Eq, const N: usize>(
        &self,
        indices: [usize; N],
        filter: Option<T>,
    ) -> Result<Vec<String<AT_VALUE_LEN>, N>, Error> {
        self.response(filter.map(|t| (t, indices[0])))?.pick_values(indices)
    }

    fn parse<T: FromStr>(s: &str) -> Result<T, Error> {
        str::parse(s).map_err(|_| Error::ParseError)
    }

    pub fn parse1<T: FromStr + Eq>(
        self,
        indices: [usize; 1],
        filter: Option<T>,
    ) -> Result<T, Error> {
        let values = self.pick_values(indices, filter)?;
        Self::parse::<T>(&values[0])
    }

    pub fn parse2<T: FromStr + Eq, U: FromStr>(
        self,
        indices: [usize; 2],
        filter: Option<T>,
    ) -> Result<(T, U), Error> {
        let values = self.pick_values(indices, filter)?;
        Ok((Self::parse::<T>(&values[0])?, Self::parse::<U>(&values[1])?))
    }

    pub fn parse3<T: FromStr + Eq, U: FromStr, V: FromStr>(
        self,
        indices: [usize; 3],
        filter: Option<T>,
    ) -> Result<(T, U, V), Error> {
        let values = self.pick_values(indices, filter)?;
        Ok((
            Self::parse::<T>(&values[0])?,
            Self::parse::<U>(&values[1])?,
            Self::parse::<V>(&values[2])?,
        ))
    }

    pub fn parse4<T: FromStr + Eq, U: FromStr, V: FromStr, W: FromStr>(
        self,
        indices: [usize; 4],
    ) -> Result<(T, U, V, W), Error> {
        let values = self.pick_values::<T, 4>(indices, None)?;
        Ok((
            Self::parse::<T>(&values[0])?,
            Self::parse::<U>(&values[1])?,
            Self::parse::<V>(&values[2])?,
            Self::parse::<W>(&values[3])?,
        ))
    }

    pub fn parse5<T: FromStr + Eq, U: FromStr, V: FromStr, W: FromStr, X: FromStr>(
        self,
        indices: [usize; 5],
    ) -> Result<(T, U, V, W, X), Error> {
        let values = self.pick_values::<T, 5>(indices, None)?;
        Ok((
            Self::parse::<T>(&values[0])?,
            Self::parse::<U>(&values[1])?,
            Self::parse::<V>(&values[2])?,
            Self::parse::<W>(&values[3])?,
            Self::parse::<X>(&values[4])?,
        ))
    }
}

#[cfg(test)]
mod test_at_utils {
    use super::*;

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
    fn test_cmd_response_split_values() -> crate::Result<()> {
        let ans = CommandResponse::split_values("1,\"item1,item2\",\"cellid\",-7,20")?;
        assert_eq!(&ans, &["1", "item1,item2", "cellid", "-7", "20"]);

        let ans = CommandResponse::split_values("1,\"item1,item2\",\"cellid");
        assert_eq!(ans.unwrap_err(), Error::ParseError);
        Ok(())
    }

    #[test]
    fn test_cmd_response_pick_values() -> crate::Result<()> {
        let response = CommandResponse::new("+CMD: 1,\"item1,item2\",12")?;
        let vals = response.pick_values([1, 2])?;
        assert_eq!(&vals.as_slice(), &["item1,item2", "12"]);
        Ok(())
    }

    #[test]
    fn test_cmd_response_parse_values() -> crate::Result<()> {
        let response = CommandResponse::new("+CMD: 8,13,21")?;
        assert_eq!(response.parse_values::<u8>()?.as_slice(), &[8, 13, 21]);
        Ok(())
    }

    #[test]
    fn test_at_response() -> crate::Result<()> {
        let mut from_modem_vec = Vec::new();
        from_modem_vec
            .push(FromModem::CommandResponse(CommandResponse::new(
                "+CONN: 1,\"disconnected\"",
            )?))
            .unwrap();
        from_modem_vec
            .push(FromModem::CommandResponse(CommandResponse::new(
                "+CONN: 5,\"connected\"",
            )?))
            .unwrap();
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
        let from_modem_vec = Vec::from_array([FromModem::CommandResponse(CommandResponse::new(
            "+CONN: 1,783,\"disconnected\"",
        )?)]);

        let at_response = AtResponse::new(from_modem_vec, "+CONN?");
        assert_eq!(at_response.count_response_values().unwrap(), 3);
        let (id, status) = at_response.parse2::<u8, String<20>>([0, 2], None).unwrap();
        assert_eq!(id, 1);
        assert_eq!(status, "disconnected");

        Ok(())
    }

    #[test]
    fn test_at_response_parse4() -> crate::Result<()> {
        let from_modem_vec = Vec::from_array([FromModem::CommandResponse(CommandResponse::new(
            "+QCSQ: \"NBIoT\",0,-131,55,-20",
        )?)]);

        let at_response = AtResponse::new(from_modem_vec, "+QCSQ");
        let (rssi_dbm, rsrp_dbm, snr_mult, rsrq_dbm) =
            at_response.parse4::<i8, i16, u8, i8>([1, 2, 3, 4]).unwrap();
        assert_eq!(rssi_dbm, 0);
        assert_eq!(rsrp_dbm, -131);
        assert_eq!(snr_mult, 55);
        assert_eq!(rsrq_dbm, -20);

        Ok(())
    }
}
