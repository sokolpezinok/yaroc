use core::option::{Option, Option::None, Option::Some};
use core::str::FromStr;
use defmt;
use heapless::{String, Vec};

use crate::error::Error;

pub fn split_at_response(line: &str) -> Option<(&str, &str)> {
    if line.starts_with('+') {
        if let Some(prefix_len) = line.find(": ") {
            let prefix = &line[1..prefix_len];
            let rest = &line[prefix_len + 2..];
            return Some((prefix, rest));
        }
    }
    None
}

#[derive(Clone, Debug)]
pub enum FromModem {
    Line(String<AT_COMMAND_SIZE>),
    Ok,
    Error,
}

impl defmt::Format for FromModem {
    fn format(&self, fmt: defmt::Formatter) {
        match self {
            FromModem::Line(line) => defmt::write!(fmt, "{}", line.as_str()),
            FromModem::Ok => defmt::write!(fmt, "Ok"),
            FromModem::Error => defmt::write!(fmt, "Error"),
        }
    }
}

pub const AT_COMMAND_SIZE: usize = 90;
pub const AT_RESPONSE_SIZE: usize = 50;
pub const AT_LINES: usize = 4;
const AT_VALUE_LEN: usize = 40;
const AT_VALUE_COUNT: usize = 4;

pub struct AtResponse {
    lines: Vec<FromModem, AT_LINES>,
    command: String<AT_COMMAND_SIZE>,
}

impl defmt::Format for AtResponse {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(fmt, "{=[?]}", self.lines.as_slice());
    }
}

impl AtResponse {
    pub fn new(lines: Vec<FromModem, AT_LINES>, command: &str) -> Self {
        Self {
            lines,
            command: String::from_str(command).unwrap(),
        }
    }

    fn parse_values(mut rest: &str) -> Vec<&str, 15> {
        let mut split = Vec::new();
        while !rest.is_empty() {
            let pos = match rest.chars().next() {
                Some('"') => {
                    let pos = rest.find("\",").unwrap_or(rest.len() - 1);
                    split.push(&rest[1..pos]).unwrap();
                    pos + 1
                }
                _ => {
                    let pos = rest.find(",").unwrap_or(rest.len());
                    split.push(&rest[..pos]).unwrap();
                    pos
                }
            };
            if pos >= rest.len() {
                break;
            }
            rest = &rest[pos + 1..];
        }
        split
    }

    fn response<T: FromStr + Eq>(
        &self,
        filter: Option<(T, usize)>,
    ) -> Result<String<AT_RESPONSE_SIZE>, Error> {
        let pos = self.command.find(['=', '?']).unwrap_or(self.command.len());
        let prefix = &self.command[..pos];
        for line in &self.lines {
            if let FromModem::Line(line) = line {
                if line.starts_with(prefix) {
                    let (_, rest) = split_at_response(line).unwrap();
                    match filter.as_ref() {
                        Some((t, idx)) => {
                            let values = Self::parse_values(rest);
                            let val: Option<T> = str::parse(&values[*idx]).ok();
                            if val.is_some() && val.unwrap() == *t {
                                return String::from_str(rest)
                                    .map_err(|_| Error::BufferTooSmallError);
                            }
                        }
                        None => {
                            return String::from_str(rest).map_err(|_| Error::BufferTooSmallError)
                        }
                    }
                }
            }
        }
        Err(Error::AtError)
    }

    // Pick values from a AT response given by the list of `indices`.
    //
    // If filter is None, the first at response is chosen. If `filter` is provided, only the response
    // for which the first chosen value (at position `indices[0]`) matches `filter`.
    fn pick_values<T: FromStr + Eq>(
        self,
        indices: &[usize],
        filter: Option<T>,
    ) -> Result<Vec<String<AT_VALUE_LEN>, AT_VALUE_COUNT>, Error> {
        let response = self.response(filter.map(|t| (t, indices[0])))?;
        let values = Self::parse_values(response.as_str());
        Ok(indices
            .iter()
            .filter_map(|idx| Some(String::from_str(values.get(*idx)?).unwrap())) //TODO
            .collect())
    }

    fn parse<T: FromStr>(s: &str) -> Result<T, Error> {
        str::parse(s).map_err(|_| Error::ParseError)
    }

    pub fn parse1<T: FromStr + Eq>(
        self,
        indices: [usize; 1],
        filter: Option<T>,
    ) -> Result<T, Error> {
        let values = self.pick_values(&indices, filter)?;
        Self::parse::<T>(&values[0])
    }

    pub fn parse2<T: FromStr + Eq, U: FromStr>(
        self,
        indices: [usize; 2],
        filter: Option<T>,
    ) -> Result<(T, U), Error> {
        let values = self.pick_values(&indices, filter)?;
        if values.len() != 2 {
            return Err(Error::ParseError);
        }
        Ok((Self::parse::<T>(&values[0])?, Self::parse::<U>(&values[1])?))
    }

    pub fn parse3<T: FromStr + Eq, U: FromStr, V: FromStr>(
        self,
        indices: [usize; 3],
    ) -> Result<(T, U, V), Error> {
        let values = self.pick_values::<T>(&indices, None)?;
        if values.len() != 3 {
            return Err(Error::ParseError);
        }
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
        let values = self.pick_values::<T>(&indices, None)?;
        if values.len() != 4 {
            return Err(Error::ParseError);
        }
        Ok((
            Self::parse::<T>(&values[0])?,
            Self::parse::<U>(&values[1])?,
            Self::parse::<V>(&values[2])?,
            Self::parse::<W>(&values[3])?,
        ))
    }
}

#[cfg(test)]
mod test_at_utils {
    use super::*;

    #[test]
    fn test_split_at_response() {
        let res = "+QMTSTAT: 0,2";
        assert_eq!(split_at_response(res), Some(("QMTSTAT", "0,2")));

        let res = "QMTSTAT: 0,2";
        assert_eq!(split_at_response(res), None);
        let res = "+QMTSTAT 0,2";
        assert_eq!(split_at_response(res), None);
    }

    #[test]
    fn test_parse_values() {
        let ans = AtResponse::parse_values("1,\"item1,item2\",\"cellid\"");
        assert_eq!(ans[0], "1");
        assert_eq!(ans[1], "item1,item2");
        assert_eq!(ans[2], "cellid");
    }

    #[test]
    fn test_response() {
        let mut from_modem_vec = Vec::new();
        from_modem_vec
            .push(FromModem::Line(
                String::from_str("+CONN: 1,disconnected").unwrap(),
            ))
            .unwrap();
        from_modem_vec
            .push(FromModem::Line(
                String::from_str("+CONN: 5,connected").unwrap(),
            ))
            .unwrap();
        let at_response = AtResponse::new(from_modem_vec, "+CONN?");
        let response = at_response.response(Some((5u8, 0))).unwrap();
        assert_eq!(response, "5,connected");
    }
}
