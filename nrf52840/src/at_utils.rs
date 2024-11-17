use core::str::{from_utf8, FromStr};
use defmt::*;
use embassy_executor::Spawner;
use embassy_nrf::peripherals::{TIMER0, UARTE1};
use embassy_nrf::uarte::{UarteRxWithIdle, UarteTx};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{with_deadline, Duration, Instant};
use heapless::{format, String, Vec};

use crate::error::Error;

pub fn split_at_response(line: &str) -> Option<(&str, &str)> {
    if line.starts_with('+') {
        let prefix_len = line.find(": ");
        if let Some(prefix_len) = prefix_len {
            let prefix = &line[1..prefix_len];
            let rest = &line[prefix_len + 2..];
            return Some((prefix, rest));
        }
    }
    None
}

#[derive(Clone)]
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

const AT_COMMAND_SIZE: usize = 90;
const AT_RESPONSE_SIZE: usize = 50;
const AT_LINES: usize = 4;
const AT_VALUE_LEN: usize = 20;
const AT_VALUE_COUNT: usize = 4;

static MAIN_CHANNEL: Channel<ThreadModeRawMutex, Result<FromModem, Error>, 5> = Channel::new();
pub static URC_CHANNEL: Channel<ThreadModeRawMutex, Result<String<AT_COMMAND_SIZE>, Error>, 2> =
    Channel::new();

async fn parse_lines(buf: &[u8], urc_handler: fn(&str, &str) -> bool) {
    let lines = from_utf8(buf)
        .map_err(|_| Error::StringEncodingError)
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty());
    let mut open_stream = false;
    for line in lines {
        let is_callback = split_at_response(line)
            .map(|(prefix, rest)| (urc_handler)(prefix, rest))
            .unwrap_or_default();

        let to_send = match line {
            "OK" | "RDY" => Ok(FromModem::Ok),
            "ERROR" => Ok(FromModem::Error),
            line => String::from_str(line)
                .map(FromModem::Line)
                .map_err(|_| Error::BufferTooSmallError),
        };
        if !is_callback {
            if let Ok(FromModem::Line(_)) = to_send.as_ref() {
                open_stream = true;
            } else {
                open_stream = false;
            }
            MAIN_CHANNEL.send(to_send).await;
        } else {
            URC_CHANNEL.send(Ok(String::from_str(line).unwrap())).await;
        }
    }
    if open_stream {
        MAIN_CHANNEL.send(Ok(FromModem::Ok)).await; // Stop transmission
    }
}

#[embassy_executor::task]
async fn reader(
    mut rx: UarteRxWithIdle<'static, UARTE1, TIMER0>,
    urc_handler: fn(&str, &str) -> bool,
) {
    const AT_BUF_SIZE: usize = 300;
    let mut buf = [0; AT_BUF_SIZE];
    loop {
        let len = rx
            .read_until_idle(&mut buf)
            .await
            .map_err(|_| Error::UartReadError);
        match len {
            Err(err) => MAIN_CHANNEL.send(Err(err)).await,
            Ok(len) => parse_lines(&buf[..len], urc_handler).await,
        }
    }
}

pub struct AtUart {
    // This struct is fixed to UARTE1 due to a limitation of embassy_executor::task. We cannot make
    // the `reader` method generic and also work for UARTE0. However, for our hardware this is not
    // needed, UARTE0 does not use AT-commands, so it won't use this struct.
    tx: UarteTx<'static, UARTE1>,
}

pub struct AtResponse {
    lines: Vec<FromModem, AT_LINES>,
    command: String<AT_COMMAND_SIZE>, //result: crate::Result<String<AT_COMMAND_SIZE>>,
}

impl defmt::Format for AtResponse {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(fmt, "{=[?]}", self.lines.as_slice());
    }
}

impl AtResponse {
    fn new(lines: Vec<FromModem, AT_LINES>, command: &str) -> Self {
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
    ) -> Option<String<AT_RESPONSE_SIZE>> {
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
                                return Some(
                                    String::from_str(rest)
                                        .map_err(|_| Error::BufferTooSmallError)
                                        .unwrap(),
                                );
                            }
                        }
                        None => {
                            return Some(
                                String::from_str(rest)
                                    .map_err(|_| Error::BufferTooSmallError)
                                    .unwrap(),
                            )
                        }
                    }
                }
            }
        }
        None
    }

    // Pick values from a AT response given by the list of `indices`.
    //
    // If filter is None, the first at response is chosen. If `filter` is provided, only the response
    // for which the first chosen value (at position `indices[0]`) matches `filter`.
    fn pick_values<T: FromStr + Eq>(
        self,
        indices: &[usize],
        filter: Option<T>,
    ) -> Option<Vec<String<AT_VALUE_LEN>, AT_VALUE_COUNT>> {
        let response = self.response(filter.map(|t| (t, indices[0])))?;
        let values = Self::parse_values(response.as_str());
        Some(
            indices
                .iter()
                .filter_map(|idx| Some(String::from_str(values.get(*idx)?).unwrap())) //TODO
                .collect(),
        )
    }

    fn parse<T: FromStr>(s: &str) -> Result<T, Error> {
        str::parse(s).map_err(|_| Error::ParseError)
    }

    pub fn parse1<T: FromStr + Eq>(
        self,
        indices: [usize; 1],
        filter: Option<T>,
    ) -> Result<T, Error> {
        let values = self.pick_values(&indices, filter).ok_or(Error::AtError)?;
        Self::parse::<T>(&values[0])
    }

    pub fn parse2<T: FromStr + Eq, U: FromStr>(
        self,
        indices: [usize; 2],
        filter: Option<T>,
    ) -> Result<(T, U), Error> {
        let values = self.pick_values(&indices, filter).ok_or(Error::AtError)?;
        if values.len() != 2 {
            return Err(Error::AtError);
        }
        Ok((Self::parse::<T>(&values[0])?, Self::parse::<U>(&values[1])?))
    }

    pub fn parse3<T: FromStr + Eq, U: FromStr, V: FromStr>(
        self,
        indices: [usize; 3],
    ) -> Result<(T, U, V), Error> {
        let values = self
            .pick_values::<T>(&indices, None)
            .ok_or(Error::AtError)?;
        if values.len() != 3 {
            return Err(Error::AtError);
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
        let values = self
            .pick_values::<T>(&indices, None)
            .ok_or(Error::AtError)?;
        if values.len() != 4 {
            return Err(Error::AtError);
        }
        Ok((
            Self::parse::<T>(&values[0])?,
            Self::parse::<U>(&values[1])?,
            Self::parse::<V>(&values[2])?,
            Self::parse::<W>(&values[3])?,
        ))
    }
}

impl AtUart {
    pub fn new(
        rx: UarteRxWithIdle<'static, UARTE1, TIMER0>,
        tx: UarteTx<'static, UARTE1>,
        urc_handler: fn(&str, &str) -> bool,
        spawner: &Spawner,
    ) -> Self {
        unwrap!(spawner.spawn(reader(rx, urc_handler)));
        Self { tx }
    }

    pub async fn read(&mut self, timeout: Duration) -> Result<Vec<FromModem, 4>, Error> {
        let mut res = Vec::new();
        let deadline = Instant::now() + timeout;
        loop {
            let from_modem = with_deadline(deadline, MAIN_CHANNEL.receive())
                .await
                .map_err(|_| Error::TimeoutError)??;
            res.push(from_modem.clone())
                .map_err(|_| Error::BufferTooSmallError)?;
            match from_modem {
                FromModem::Ok | FromModem::Error => break,
                _ => {}
            }
        }

        Ok(res)
    }

    async fn write(&mut self, command: &str) -> Result<(), Error> {
        let command = format!(AT_COMMAND_SIZE; "AT{command}\r").unwrap();

        self.tx
            .write(command.as_bytes())
            .await
            .map_err(|_| Error::UartWriteError)
    }

    async fn call_impl(
        &mut self,
        command: &str,
        timeout: Duration,
    ) -> Result<Vec<FromModem, AT_LINES>, Error> {
        self.write(command).await?;
        let lines = self.read(timeout).await?;
        match lines.last() {
            Some(&FromModem::Ok) => Ok(lines),
            Some(&FromModem::Error) => {
                error!(
                    "Failed response from modem: {} {=[?]}",
                    command,
                    lines.as_slice()
                );
                Err(Error::AtErrorResponse)
            }
            _ => {
                error!(
                    "Failed response from modem: {} {=[?]}",
                    command,
                    lines.as_slice()
                );
                Err(Error::AtError)
            }
        }
    }

    pub async fn call(&mut self, command: &str, timeout: Duration) -> Result<AtResponse, Error> {
        let start = Instant::now();
        let lines = self.call_impl(command, timeout).await?;
        let response = AtResponse::new(lines, command);
        debug!(
            "{}: {}, took {}ms",
            command,
            response,
            (Instant::now() - start).as_millis()
        );
        Ok(response)
    }

    pub async fn call_with_response(
        &mut self,
        command: &str,
        call_timeout: Duration,
        response_timeout: Duration,
    ) -> Result<AtResponse, Error> {
        let start = Instant::now();
        let mut lines = self.call_impl(command, call_timeout).await?;
        lines.extend(self.read(response_timeout).await?);
        let response = AtResponse::new(lines, command);
        debug!(
            "{}: {}, took {}ms",
            command,
            response,
            (Instant::now() - start).as_millis()
        );
        Ok(response)
    }
}

//#[cfg(test)]
//mod test_at_utils {
//    #[test]
//    fn test_split_at_response() {
//        let res = "+QMTSTAT: 0,2";
//        assert_eq!(split_at_response(res), Some(("QMTSTAT", "0,2")));
//
//        let res = "QMTSTAT: 0,2";
//        assert_eq!(split_at_response(res), None);
//        let res = "+QMTSTAT 0,2";
//        assert_eq!(split_at_response(res), None);
//    }
//}
