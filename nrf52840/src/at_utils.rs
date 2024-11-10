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

const AT_COMMAND_SIZE: usize = 100;

pub struct AtUart {
    tx: UarteTx<'static, UARTE1>,
    #[allow(dead_code)]
    callback_dispatcher: fn(&str, &str) -> bool,
}

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
                for line in lines {
                    let mut is_callback = false;
                    if line.starts_with('+') {
                        let prefix_len = line.find(": ");
                        if let Some(prefix_len) = prefix_len {
                            let prefix = &line[1..prefix_len];
                            let rest = &line[prefix_len + 2..];
                            if (callback_dispatcher)(prefix, rest) {
                                is_callback = true;
                            }
                        }
                    }

                    if !is_callback {
                        CHANNEL
                            .send(String::from_str(line).map_err(|_| Error::StringEncodingError))
                            .await;
                    }
                }
                CHANNEL.send(Ok(String::new())).await; // Stop transmission
            }
        }
    }
}

impl AtUart {
    pub fn new(
        rx: UarteRxWithIdle<'static, UARTE1, TIMER0>,
        tx: UarteTx<'static, UARTE1>,
        callback_dispatcher: fn(&str, &str) -> bool,
        spawner: &Spawner,
    ) -> Self {
        unwrap!(spawner.spawn(reader(rx, callback_dispatcher)));
        Self {
            tx,
            callback_dispatcher,
        }
    }

    pub async fn read(&mut self, timeout: Duration) -> Result<(), Error> {
        loop {
            let read_fut = CHANNEL.receive();
            let line = with_timeout(timeout, read_fut)
                .await
                .map_err(|_| Error::TimeoutError)?;
            let line = line?;
            if line.is_empty() {
                break;
            }

            info!("Read {}", line.as_str());
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
