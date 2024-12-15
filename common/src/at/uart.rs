use super::response::{AtResponse, CommandResponse, FromModem, AT_COMMAND_SIZE, AT_LINES};
use core::option::Option::Some;
use core::str::FromStr;
#[cfg(feature = "defmt")]
use defmt::{self, debug, info};
use embassy_executor::Spawner;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, WithTimeout};
use heapless::{format, String, Vec};
#[cfg(not(feature = "defmt"))]
use log::debug;

use crate::{error::Error, RawMutex};

pub type MainRxChannelType = Channel<RawMutex, Result<FromModem, Error>, 5>;
pub static MAIN_RX_CHANNEL: MainRxChannelType = Channel::new();

/// A broker of AT replies (listening to UART RX) and routing each reply either to the main channel
/// or channel dedicated to URCs.
pub struct AtRxBroker {
    main_channel: &'static MainRxChannelType,
}

impl AtRxBroker {
    pub fn new(main_channel: &'static MainRxChannelType) -> Self {
        Self { main_channel }
    }

    /// Parse lines out of a given text and forward each line to the appropriate channel.
    ///
    /// `urc_handler`: returns true if the command response is a URC and has been handled by the
    /// handler.
    async fn parse_lines(&self, text: &str, urc_handler: fn(&CommandResponse) -> bool) {
        let lines = text.lines().filter(|line| !line.is_empty());
        let mut open_stream = false;
        for line in lines {
            let to_send = match line {
                "OK" | "RDY" | "APP RDY" | "> " => Ok(FromModem::Ok),
                "ERROR" => Ok(FromModem::Error),
                line => {
                    if let Ok(command_response) = CommandResponse::new(line) {
                        Ok(FromModem::CommandResponse(command_response))
                    } else {
                        String::from_str(line)
                            .map(FromModem::Line)
                            .map_err(|_| Error::BufferTooSmallError)
                    }
                }
            };

            if let Ok(FromModem::CommandResponse(command_response)) = to_send.as_ref() {
                if urc_handler(command_response) {
                    #[cfg(feature = "defmt")]
                    info!("Got URC {}", line);
                    continue;
                }
            }

            if let Ok(from_modem) = to_send.as_ref() {
                open_stream = !from_modem.terminal();
            } else {
                open_stream = false;
            }
            self.main_channel.send(to_send).await;
        }
        if open_stream {
            self.main_channel.send(Ok(FromModem::Eof)).await; // Stop transmission
        }
    }

    pub async fn broker_loop<R: RxWithIdle>(
        mut rx: R,
        urc_handler: fn(&CommandResponse) -> bool,
        main_rx_channel: &'static MainRxChannelType,
    ) {
        const AT_BUF_SIZE: usize = 300;
        let mut buf = [0; AT_BUF_SIZE];
        let at_broker = AtRxBroker::new(main_rx_channel);
        loop {
            let len = rx.read_until_idle(&mut buf).await;
            match len {
                Err(err) => main_rx_channel.send(Err(err)).await,
                Ok(len) => {
                    let text = core::str::from_utf8(&buf[..len]);
                    match text {
                        Err(_) => main_rx_channel.send(Err(Error::StringEncodingError)).await,
                        Ok(text) => at_broker.parse_lines(text, urc_handler).await,
                    }
                }
            }
        }
    }
}

pub trait RxWithIdle {
    /// Spawn a new task on `spawner` that reads RX from UART and clasifies answers using
    /// `urc_handler`.
    fn spawn(self, spawner: &Spawner, urc_handler: fn(&CommandResponse) -> bool);

    /// Read from UART until it's idle. Return the number of read bytes.
    fn read_until_idle(
        &mut self,
        buf: &mut [u8],
    ) -> impl core::future::Future<Output = crate::Result<usize>>;
}

pub trait Tx {
    fn write(&mut self, buffer: &[u8]) -> impl core::future::Future<Output = crate::Result<()>>;
}

pub struct AtUart<T: Tx> {
    tx: T,
    main_rx_channel: &'static MainRxChannelType,
}

impl<T: Tx> AtUart<T> {
    pub fn new<R: RxWithIdle>(
        rx: R,
        tx: T,
        urc_handler: fn(&CommandResponse) -> bool,
        spawner: &Spawner,
    ) -> Self {
        rx.spawn(spawner, urc_handler);
        Self {
            tx,
            main_rx_channel: &MAIN_RX_CHANNEL,
        }
    }

    pub async fn read(&self, timeout: Duration) -> Result<Vec<FromModem, 4>, Error> {
        let mut res = Vec::new();
        let deadline = Instant::now() + timeout;
        loop {
            let from_modem = self
                .main_rx_channel
                .receive()
                .with_deadline(deadline)
                .await
                .map_err(|_| Error::TimeoutError)??;
            res.push(from_modem.clone()).map_err(|_| Error::BufferTooSmallError)?;
            if from_modem.terminal() {
                break;
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
        debug!("Calling: {}", command);
        self.write_at(command).await?;
        let lines = self.read(timeout).await?;
        match lines.last() {
            Some(&FromModem::Ok) => Ok(lines),
            Some(&FromModem::Error) => {
                #[cfg(feature = "defmt")]
                debug!(
                    "Failed response from modem: {} {=[?]}",
                    command,
                    lines.as_slice()
                );
                Err(Error::AtErrorResponse)
            }
            _ => {
                #[cfg(feature = "defmt")]
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

#[cfg(test)]
mod test_at {
    use super::*;
    use embassy_futures::block_on;

    #[test]
    fn test_at_broker() -> crate::Result<()> {
        static MAIN_RX_CHANNEL: MainRxChannelType = Channel::new();
        static URC_CHANNEL: Channel<RawMutex, CommandResponse, 1> = Channel::new();
        let broker = AtRxBroker::new(&MAIN_RX_CHANNEL);
        let handler = |response: &CommandResponse| match response.command() {
            "URC" => {
                URC_CHANNEL.try_send(response.clone()).unwrap();
                true
            }
            _ => false,
        };

        block_on(broker.parse_lines("OK\r\n+URC: 1,\"string\"\nERROR", handler));
        assert_eq!(MAIN_RX_CHANNEL.try_receive().unwrap()?, FromModem::Ok);
        let urc = URC_CHANNEL.try_receive().unwrap();
        assert_eq!(urc.command(), "URC");
        assert_eq!(urc.values().as_slice(), ["1", "string"]);
        assert_eq!(MAIN_RX_CHANNEL.try_receive().unwrap()?, FromModem::Error);

        let long = "123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890X";
        block_on(broker.parse_lines(long, handler));
        assert_eq!(
            MAIN_RX_CHANNEL.try_receive().unwrap(),
            Err(Error::BufferTooSmallError)
        );

        block_on(broker.parse_lines("+NONURC: 1\n", handler));
        assert_eq!(
            MAIN_RX_CHANNEL.try_receive().unwrap()?,
            FromModem::CommandResponse(CommandResponse::new("+NONURC: 1")?)
        );
        assert_eq!(MAIN_RX_CHANNEL.try_receive().unwrap()?, FromModem::Eof);
        assert_eq!(MAIN_RX_CHANNEL.len(), 0);
        Ok(())
    }
}
