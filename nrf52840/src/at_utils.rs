use core::str::from_utf8;
use defmt::*;
use embassy_nrf::peripherals::TIMER0;
use embassy_nrf::uarte::{self, UarteRxWithIdle, UarteTx};
use embassy_time::{with_timeout, Duration};
use heapless::{String, Vec};

use crate::error::Error;

const AT_BUF_SIZE: usize = 300;
const AT_COMMAND_SIZE: usize = 100;

pub struct Uart<'a, UartType: uarte::Instance> {
    rx: UarteRxWithIdle<'a, UartType, TIMER0>,
    tx: UarteTx<'a, UartType>,
    #[allow(dead_code)]
    callback_dispatcher: fn(&str, &str) -> bool,
}

impl<'a, UartType: uarte::Instance> Uart<'a, UartType> {
    pub fn new(
        rx: UarteRxWithIdle<'a, UartType, TIMER0>,
        tx: UarteTx<'a, UartType>,
        callback_dispatcher: fn(&str, &str) -> bool,
    ) -> Self {
        Self {
            rx,
            tx,
            callback_dispatcher,
        }
    }

    pub async fn read(&mut self, timeout: Duration) -> Result<(), Error> {
        let mut buf = [0; AT_BUF_SIZE];
        let read_fut = self.rx.read_until_idle(&mut buf);
        let len = with_timeout(timeout, read_fut)
            .await
            .map_err(|_| Error::TimeoutError)?
            .map_err(|_| Error::UartReadError)?;

        let lines = split_lines(&buf[..len])?;
        for line in lines {
            info!("Read {}", line);
        }

        Ok(())
    }

    pub async fn call(&mut self, command: &str, timeout: Duration) -> Result<(), Error> {
        let mut command: String<AT_COMMAND_SIZE> = String::try_from(command).unwrap();
        command.push('\r').unwrap();

        self.tx
            .write(command.as_bytes())
            .await
            .map_err(|_| Error::UartWriteError)?;

        self.read(timeout).await
    }

    async fn _dispatch_response(self, line: &str) {
        if line.starts_with('+') {
            let prefix_len = line.find(": ");
            if let Some(prefix_len) = prefix_len {
                let prefix = &line[1..prefix_len];
                let rest = &line[prefix_len..];
                if !(self.callback_dispatcher)(prefix, rest) {
                    // TODO: forward
                }
            }
        }
    }
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
//}
