use core::str::{from_utf8, FromStr};
use defmt::*;
use embassy_executor::Spawner;
use embassy_nrf::peripherals::{TIMER0, UARTE1};
use embassy_nrf::uarte::{UarteRxWithIdle, UarteTx};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{with_timeout, Duration};
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

fn readline(s: &str) -> (Option<&str>, &str) {
    match s.find("\r\n") {
        None => (None, s),
        Some(len) => (Some(&s[..len]), &s[len + 2..]),
    }
}

pub fn split_lines(buf: &[u8]) -> Result<Vec<&str, 10>, Error> {
    let mut lines = Vec::new();
    let mut s = from_utf8(buf).map_err(|_| Error::StringEncodingError)?;
    while let (Some(line), rest) = readline(s) {
        s = rest;
        if line.is_empty() {
            continue;
        }
        lines.push(line).map_err(|_| Error::BufferTooSmallError)?;
    }
    Ok(lines)
}

const AT_COMMAND_SIZE: usize = 100;

static CHANNEL: Channel<ThreadModeRawMutex, Result<String<AT_COMMAND_SIZE>, Error>, 5> =
    Channel::new();

#[embassy_executor::task]
async fn reader(
    mut rx: UarteRxWithIdle<'static, UARTE1, TIMER0>,
    callback_dispatcher: fn(&str, &str) -> bool,
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
            Ok(len) => {
                let lines = split_lines(&buf[..len]).unwrap();
                let mut lines_count = 0;
                for (idx, line) in lines.iter().enumerate() {
                    let is_callback = split_at_response(line)
                        .map(|(prefix, rest)| (callback_dispatcher)(prefix, rest))
                        .unwrap_or_default();

                    if !is_callback {
                        CHANNEL
                            .send(String::from_str(line).map_err(|_| Error::StringEncodingError))
                            .await;
                        debug!("Read: {}", line);
                        lines_count += 1;
                        if (*line == "OK" || *line == "ERROR") && idx + 1 < lines.len() {
                            CHANNEL.send(Ok(String::new())).await; // Mark a finished command
                            lines_count = 0;
                        }
                    } else {
                        info!("CALLBACK! {}", line);
                    }
                }
                if lines_count > 0 {
                    CHANNEL.send(Ok(String::new())).await; // Stop transmission
                }
            }
        }
    }
}

pub struct AtUart {
    // This struct is fixed to UARTE1 due to a limitation of embassy_executor::task. We cannot make
    // the `reader` method generic and also work for UARTE0. However, for our hardware this is not
    // needed, UARTE0 does not use AT-commands, so it won't use this struct.
    tx: UarteTx<'static, UARTE1>,
}

type Response = Vec<String<AT_COMMAND_SIZE>, 4>;

impl AtUart {
    pub fn new(
        rx: UarteRxWithIdle<'static, UARTE1, TIMER0>,
        tx: UarteTx<'static, UARTE1>,
        callback_dispatcher: fn(&str, &str) -> bool,
        spawner: &Spawner,
    ) -> Self {
        unwrap!(spawner.spawn(reader(rx, callback_dispatcher)));
        Self { tx }
    }

    pub async fn read(&mut self, timeout: Duration) -> Result<Response, Error> {
        let mut res = Vec::new();
        loop {
            let line = with_timeout(timeout, CHANNEL.receive())
                .await
                .map_err(|_| Error::TimeoutError)??;
            if line.is_empty() {
                break;
            }

            res.push(line).map_err(|_| Error::BufferTooSmallError)?;
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

    pub async fn call(&mut self, command: &str, timeout: Duration) -> Result<Response, Error> {
        self.write(command).await?;
        debug!("{}", command);
        let res = self.read(timeout).await?;
        if let Some("OK") = res.last().map(String::as_str) {
            Ok(res)
        } else {
            for line in res {
                error!("{}", line.as_str());
            }
            Err(Error::AtError)
        }
    }

    pub async fn call_with_response(
        &mut self,
        command: &str,
        call_timeout: Duration,
        response_timeout: Duration,
    ) -> Result<Response, Error> {
        // TODO: do minimum timeout here
        let mut first = self.call(command, call_timeout).await?;
        let second = self.read(response_timeout).await?;
        first.extend(second);
        Ok(first)
    }
}

//#[cfg(test)]
//mod test_at_utils {
//    use super::{readline, split_lines};
//
//    #[test]
//    fn test_readline() {
//        let s = "+MSG: hello\r\nOK\r\n";
//        let (l1, s) = readline(s);
//        let (l2, s) = readline(s);
//        let (l3, _) = readline(s);
//        assert_eq!(l1, Some("+MSG: hello"));
//        assert_eq!(l2, Some("OK"));
//        assert_eq!(l3, None);
//    }
//
//    #[test]
//    fn test_split_lines() {
//        let s = "hello\r\n\r\nOK\r\n";
//        let lines = split_lines(s.as_bytes()).unwrap();
//        assert_eq!(*lines, ["hello", "OK"]);
//    }
//
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
