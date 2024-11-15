use core::str::{from_utf8, FromStr};
use defmt::*;
use embassy_executor::Spawner;
use embassy_nrf::peripherals::{TIMER0, UARTE1};
use embassy_nrf::uarte::{UarteRxWithIdle, UarteTx};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{with_deadline, Duration, Instant};
use heapless::{String, Vec};

use crate::error::Error;

fn split_at_response(line: &str) -> Option<(&str, &str)> {
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

fn pick_values<'a>(values: &'a str, indices: &[usize]) -> Vec<String<AT_VALUE_SIZE>, 3> {
    let split: Vec<&str, 10> = values.split(',').collect();
    indices
        .into_iter()
        .filter_map(|idx| Some(String::from_str(split.get(*idx)?).unwrap())) //TODO
        .collect()
}

const AT_COMMAND_SIZE: usize = 100;
const AT_LINES: usize = 4;
const AT_VALUE_SIZE: usize = 20;

static CHANNEL: Channel<ThreadModeRawMutex, Result<FromModem, Error>, 5> = Channel::new();

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

        if !is_callback {
            let to_send = match line {
                "OK" => Ok(FromModem::Ok),
                "ERROR" => Err(Error::AtError),
                line => String::from_str(line)
                    .map(|l| FromModem::Line(l))
                    .map_err(|_| Error::StringEncodingError),
            };
            if let Ok(FromModem::Line(_)) = to_send.as_ref() {
                open_stream = true;
            } else {
                open_stream = false;
            }
            CHANNEL.send(to_send).await;
        } else {
            info!("CALLBACK! {}", line);
        }
    }
    if open_stream {
        CHANNEL.send(Ok(FromModem::Ok)).await; // Stop transmission
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
            Err(err) => CHANNEL.send(Err(err)).await,
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
    answer: Result<Vec<String<AT_VALUE_SIZE>, 3>, Error>,
}

impl defmt::Format for AtResponse {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(fmt, "{=[?]}", self.lines.as_slice());
    }
}

pub trait Parseable: Sized {
    fn parse(s: &String<AT_VALUE_SIZE>) -> Result<Self, Error>;
}

impl Parseable for u32 {
    fn parse(s: &String<AT_VALUE_SIZE>) -> Result<Self, Error> {
        str::parse(s.as_str()).map_err(|_| Error::ParseError)
    }
}

impl Parseable for i32 {
    fn parse(s: &String<AT_VALUE_SIZE>) -> Result<Self, Error> {
        str::parse(s.as_str()).map_err(|_| Error::ParseError)
    }
}

impl<const N: usize> Parseable for String<N> {
    fn parse(s: &String<AT_VALUE_SIZE>) -> Result<Self, Error> {
        String::from_str(&s[1..s.len() - 1]).map_err(|_| Error::ParseError)
    }
}

impl AtResponse {
    fn new(lines: Vec<FromModem, AT_LINES>, command: &str, indices: &[usize]) -> Self {
        let pos = command.find(['=', '?']).unwrap_or(command.len());
        let prefix = &command[2..pos];
        for line in &lines {
            if let FromModem::Line(line) = line {
                if line.starts_with(prefix) {
                    let (_, rest) = split_at_response(line).unwrap();
                    let values = pick_values(rest, indices);
                    return Self {
                        answer: Ok(values),
                        lines,
                    };
                }
            }
        }
        Self {
            lines,
            answer: Err(Error::AtError), // TODO: different return type
        }
    }

    pub fn parse1<T: Parseable>(self) -> Result<T, Error> {
        let values = self.answer?;
        if values.len() != 1 {
            return Err(Error::AtError);
        }
        T::parse(&values[0])
    }

    pub fn parse2<T: Parseable, U: Parseable>(self) -> Result<(T, U), Error> {
        let values = self.answer?;
        if values.len() != 2 {
            return Err(Error::AtError);
        }
        Ok((T::parse(&values[0])?, U::parse(&values[1])?))
    }

    pub fn parse3<T: Parseable, U: Parseable, V: Parseable>(self) -> Result<(T, U, V), Error> {
        let values = self.answer?;
        if values.len() != 3 {
            return Err(Error::AtError);
        }
        Ok((
            T::parse(&values[0])?,
            U::parse(&values[1])?,
            V::parse(&values[2])?,
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

    async fn read(&mut self, timeout: Duration) -> Result<Vec<FromModem, 4>, Error> {
        let mut res = Vec::new();
        let deadline = Instant::now() + timeout;
        loop {
            let from_modem = with_deadline(deadline, CHANNEL.receive())
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
        let mut command: String<AT_COMMAND_SIZE> = String::try_from(command).unwrap();
        command.push('\r').unwrap();

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
        debug!("Calling {}", command);
        self.write(command).await?;
        let lines = self.read(timeout).await?;
        if let Some(&FromModem::Ok) = lines.last() {
            Ok(lines)
        } else {
            Err(Error::AtError)
        }
    }

    pub async fn call(
        &mut self,
        command: &str,
        timeout: Duration,
        indices: &[usize],
    ) -> Result<AtResponse, Error> {
        let lines = self.call_impl(command, timeout).await?;
        let response = AtResponse::new(lines, command, indices);
        debug!("Got: {}", response);
        Ok(response)
    }

    pub async fn call_with_response(
        &mut self,
        command: &str,
        call_timeout: Duration,
        response_timeout: Duration,
        indices: &[usize],
    ) -> Result<AtResponse, Error> {
        let mut lines = self.call_impl(command, call_timeout).await?;
        lines.extend(self.read(response_timeout).await?);
        let response = AtResponse::new(lines, command, indices);
        debug!("Got: {}", response);
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
