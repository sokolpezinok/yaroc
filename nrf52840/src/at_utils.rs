use common::at::{split_at_response, AtResponse, FromModem, AT_COMMAND_SIZE, AT_LINES};
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

static MAIN_CHANNEL: MainChannelType = Channel::new();
pub static URC_CHANNEL: UrcChannelType = Channel::new();

type MainChannelType = Channel<ThreadModeRawMutex, Result<FromModem, Error>, 5>;
type UrcChannelType = Channel<ThreadModeRawMutex, Result<String<AT_COMMAND_SIZE>, Error>, 2>;

pub struct AtBroker {
    main_channel: &'static MainChannelType,
    urc_channel: &'static UrcChannelType,
}

impl AtBroker {
    pub fn new(
        main_channel: &'static MainChannelType,
        urc_channel: &'static UrcChannelType,
    ) -> Self {
        Self {
            main_channel,
            urc_channel,
        }
    }

    async fn parse_lines(&self, text: &str, urc_handler: fn(&str, &str) -> bool) {
        let lines = text.lines().filter(|line| !line.is_empty());
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
                self.main_channel.send(to_send).await;
            } else {
                self.urc_channel.send(Ok(String::from_str(line).unwrap())).await;
            }
        }
        if open_stream {
            self.main_channel.send(Ok(FromModem::Ok)).await; // Stop transmission
        }
    }
}

#[embassy_executor::task]
async fn reader(
    mut rx: UarteRxWithIdle<'static, UARTE1, TIMER0>,
    urc_classifier: fn(&str, &str) -> bool,
) {
    const AT_BUF_SIZE: usize = 300;
    let mut buf = [0; AT_BUF_SIZE];
    let at_broker = AtBroker::new(&MAIN_CHANNEL, &URC_CHANNEL);
    loop {
        let len = rx.read_until_idle(&mut buf).await.map_err(|_| Error::UartReadError);
        match len {
            Err(err) => MAIN_CHANNEL.send(Err(err)).await,
            Ok(len) => {
                let text = from_utf8(&buf[..len]);
                match text {
                    Err(_) => MAIN_CHANNEL.send(Err(Error::StringEncodingError)).await,
                    Ok(text) => at_broker.parse_lines(text, urc_classifier).await,
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

impl AtUart {
    pub fn new(
        rx: UarteRxWithIdle<'static, UARTE1, TIMER0>,
        tx: UarteTx<'static, UARTE1>,
        urc_classifier: fn(&str, &str) -> bool,
        spawner: &Spawner,
    ) -> Self {
        unwrap!(spawner.spawn(reader(rx, urc_classifier)));
        Self { tx }
    }

    pub async fn read(&self, timeout: Duration) -> Result<Vec<FromModem, 4>, Error> {
        let mut res = Vec::new();
        let deadline = Instant::now() + timeout;
        loop {
            let from_modem = with_deadline(deadline, MAIN_CHANNEL.receive())
                .await
                .map_err(|_| Error::TimeoutError)??;
            res.push(from_modem.clone()).map_err(|_| Error::BufferTooSmallError)?;
            match from_modem {
                FromModem::Ok | FromModem::Error => break,
                _ => {}
            }
        }

        Ok(res)
    }

    async fn write_at(&mut self, command: &str) -> Result<(), Error> {
        let command = format!(AT_COMMAND_SIZE; "AT{command}\r")?;
        self.write(command.as_bytes()).await
    }

    async fn write(&mut self, message: &[u8]) -> crate::Result<()> {
        self.tx.write(message).await.map_err(|_| Error::UartWriteError)
    }

    pub async fn call(&mut self, message: &[u8], timeout: Duration) -> crate::Result<()> {
        self.write(message).await?;
        let lines = self.read(timeout).await?;
        let _response = AtResponse::new(lines, "");
        Ok(())
    }

    async fn call_at_impl(
        &mut self,
        command: &str,
        timeout: Duration,
    ) -> Result<Vec<FromModem, AT_LINES>, Error> {
        //debug!("Calling: {}", command);
        self.write_at(command).await?;
        let lines = self.read(timeout).await?;
        match lines.last() {
            Some(&FromModem::Ok) => Ok(lines),
            Some(&FromModem::Error) => {
                debug!(
                    "Failed response from modem: {} {=[?]}",
                    command,
                    lines.as_slice()
                );
                Err(Error::AtErrorResponse)
            }
            _ => {
                debug!(
                    "Failed response from modem: {} {=[?]}",
                    command,
                    lines.as_slice()
                );
                Err(Error::ModemError)
            }
        }
    }

    pub async fn call_at(&mut self, command: &str, timeout: Duration) -> Result<AtResponse, Error> {
        let start = Instant::now();
        let lines = self.call_at_impl(command, timeout).await?;
        let response = AtResponse::new(lines, command);
        debug!(
            "{}: {}, took {}ms",
            command,
            response,
            (Instant::now() - start).as_millis()
        );
        Ok(response)
    }

    pub async fn call_at_with_response(
        &mut self,
        command: &str,
        call_timeout: Duration,
        response_timeout: Duration,
    ) -> Result<AtResponse, Error> {
        let start = Instant::now();
        let mut lines = self.call_at_impl(command, call_timeout).await?;
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
