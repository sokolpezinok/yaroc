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
                let lines = from_utf8(&buf[..len])
                    .map_err(|_| Error::StringEncodingError)
                    .unwrap()
                    .lines();
                let mut lines_count = 0;
                for line in lines {
                    if line.is_empty() {
                        continue;
                    }
                    let is_callback = split_at_response(line)
                        .map(|(prefix, rest)| (callback_dispatcher)(prefix, rest))
                        .unwrap_or_default();

                    if !is_callback {
                        CHANNEL
                            .send(String::from_str(line).map_err(|_| Error::StringEncodingError))
                            .await;
                        lines_count += 1;
                        if line == "OK" || line == "ERROR" {
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

pub struct AtResponse {
    lines: Vec<String<AT_COMMAND_SIZE>, 4>,
    answer: Result<String<AT_COMMAND_SIZE>, Error>,
}

impl AtResponse {
    fn just_lines(lines: Vec<String<AT_COMMAND_SIZE>, 4>) -> Self {
        AtResponse {
            lines,
            answer: Ok(String::new()), // TODO: enum?
        }
    }

    fn new(lines: Vec<String<AT_COMMAND_SIZE>, 4>, command: &str) -> Self {
        let pos = command.find(['=', '?']).unwrap_or(command.len());
        let prefix = &command[2..pos];
        for line in &lines {
            if line.starts_with(prefix) {
                info!("RETURN: {}", line.as_str());
                return Self {
                    answer: Ok(line.clone()),
                    lines,
                };
            }
        }
        Self {
            lines,
            answer: Err(Error::AtError), // TODO: different return type
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
        Self { tx }
    }

    pub async fn read(
        &mut self,
        timeout: Duration,
    ) -> Result<Vec<String<AT_COMMAND_SIZE>, 4>, Error> {
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

    pub async fn call(&mut self, command: &str, timeout: Duration) -> Result<AtResponse, Error> {
        self.write(command).await?;
        debug!("{}", command);
        let lines = self.read(timeout).await?;
        if let Some("OK") = lines.last().map(String::as_str) {
            Ok(AtResponse::new(lines, command))
        } else {
            error!("Fail: {}", command);
            for line in lines {
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
    ) -> Result<AtResponse, Error> {
        self.write(command).await?;
        debug!("{}", command);
        let mut lines = self.read(call_timeout).await?;
        if let Some("OK") = lines.last().map(String::as_str) {
        } else {
            return Err(Error::AtError);
        }
        let second = self.read(response_timeout).await?;
        lines.extend(second);
        Ok(AtResponse::new(lines, command))
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
