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
    urc_handler: fn(CommandResponse) -> bool,
}

impl AtRxBroker {
    pub fn new(
        main_channel: &'static MainRxChannelType,
        urc_handler: fn(CommandResponse) -> bool,
    ) -> Self {
        Self {
            main_channel,
            urc_handler,
        }
    }

    /// Parse lines out of a given text and forward each line to the appropriate channel.
    ///
    /// `text`: the text to be parsed.
    /// `urc_handler`: returns true if the command response is a URC and has been handled by the
    /// handler.
    async fn parse_lines(&self, text: &str) {
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
                if (self.urc_handler)(command_response.clone()) {
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

    /// A loop running AtRxBroker forever.
    ///
    /// Reads all bytes from `rx` until it's idle and parses the bytes into lines ('\r\n` or `\n`
    /// are both accepted).
    ///
    /// Used mainly to plug into a `embassy_executor::task`.
    pub async fn broker_loop(&self, mut rx: impl RxWithIdle) {
        const AT_BUF_SIZE: usize = 300;
        let mut buf = [0; AT_BUF_SIZE];
        loop {
            let len = rx.read_until_idle(&mut buf).await;
            match len {
                Err(err) => self.main_channel.send(Err(err)).await,
                Ok(len) => {
                    let text = core::str::from_utf8(&buf[..len]);
                    match text {
                        Err(_) => self.main_channel.send(Err(Error::StringEncodingError)).await,
                        Ok(text) => self.parse_lines(text).await,
                    }
                }
            }
        }
    }
}

pub trait RxWithIdle {
    /// Spawn a new task on `spawner` that reads RX from UART and clasifies answers using
    /// `urc_handler`.
    fn spawn(self, spawner: &Spawner, urc_handler: fn(CommandResponse) -> bool);

    /// Read from UART until it's idle. Return the number of read bytes.
    fn read_until_idle(
        &mut self,
        buf: &mut [u8],
    ) -> impl core::future::Future<Output = crate::Result<usize>>;
}

pub trait Tx {
    /// Write bytes to the TX part of UART.
    fn write(&mut self, buffer: &[u8]) -> impl core::future::Future<Output = crate::Result<()>>;
}

pub type TxChannelType = Channel<RawMutex, String<AT_COMMAND_SIZE>, 5>;

/// Fake RxWithIdle, to be used in tests.
pub struct FakeRxWithIdle {
    responses: Vec<(&'static str, &'static str), 10>,
    tx_channel: &'static TxChannelType,
}

impl FakeRxWithIdle {
    pub fn new(
        responses: Vec<(&'static str, &'static str), 10>,
        tx_channel: &'static TxChannelType,
    ) -> Self {
        Self {
            responses,
            tx_channel,
        }
    }
}

#[embassy_executor::task]
async fn reader(rx: FakeRxWithIdle, at_broker: AtRxBroker) {
    at_broker.broker_loop(rx).await;
}

impl RxWithIdle for FakeRxWithIdle {
    fn spawn(self, spawner: &Spawner, urc_handler: fn(CommandResponse) -> bool) {
        let at_broker = AtRxBroker::new(&MAIN_RX_CHANNEL, urc_handler);
        spawner.must_spawn(reader(self, at_broker));
    }

    async fn read_until_idle(&mut self, buf: &mut [u8]) -> crate::Result<usize> {
        let recv_command = self.tx_channel.receive().await;
        if let Some((command, response)) = self.responses.first() {
            assert_eq!(command, &recv_command);
            let bytes = response.as_bytes();
            buf[..bytes.len()].clone_from_slice(bytes);
            self.responses.remove(0);
            Ok(bytes.len())
        } else {
            Err(Error::TimeoutError)
        }
    }
}

/// Fake `Tx` struct, to be used in tests
pub struct FakeTx {
    channel: &'static TxChannelType,
}

impl FakeTx {
    pub fn new(channel: &'static TxChannelType) -> FakeTx {
        Self { channel }
    }
}

impl Tx for FakeTx {
    async fn write(&mut self, buffer: &[u8]) -> crate::Result<()> {
        let s = core::str::from_utf8(buffer).map_err(|_| Error::StringEncodingError)?;
        let s = String::from_str(s).map_err(|_| Error::BufferTooSmallError)?;
        self.channel.send(s).await;
        Ok(())
    }
}

/// AT UART struct.
///
/// The TX part is represented by Tx trait, the RX part is represented by a channel of
/// type `MainRxChannelType`.
pub struct AtUart<T: Tx> {
    tx: T,
    main_rx_channel: &'static MainRxChannelType,
}

impl<T: Tx> AtUart<T> {
    pub fn new(
        rx: impl RxWithIdle,
        tx: T,
        urc_handler: fn(CommandResponse) -> bool,
        spawner: &Spawner,
    ) -> Self {
        rx.spawn(spawner, urc_handler);
        Self {
            tx,
            main_rx_channel: &MAIN_RX_CHANNEL,
        }
    }

    pub async fn read(&self, timeout: Duration) -> Result<Vec<FromModem, AT_LINES>, Error> {
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

    pub async fn call(
        &mut self,
        msg: &[u8],
        timeout: Duration,
    ) -> crate::Result<Vec<FromModem, AT_LINES>> {
        self.write(msg).await?;
        self.read(timeout).await
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

    pub async fn call_at(
        &mut self,
        command: &str,
        call_timeout: Duration,
        response_timeout: Option<Duration>,
    ) -> Result<AtResponse, Error> {
        let start = Instant::now();
        let mut lines = self.call_at_impl(command, call_timeout).await?;
        if let Some(response_timeout) = response_timeout {
            lines.extend(self.read(response_timeout).await?);
        }
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
        let handler = |response: CommandResponse| match response.command() {
            "URC" => {
                URC_CHANNEL.try_send(response.clone()).unwrap();
                true
            }
            _ => false,
        };
        let broker = AtRxBroker::new(&MAIN_RX_CHANNEL, handler);

        block_on(broker.parse_lines("OK\r\n+URC: 1,\"string\"\nERROR"));
        assert_eq!(MAIN_RX_CHANNEL.try_receive().unwrap()?, FromModem::Ok);
        let urc = URC_CHANNEL.try_receive().unwrap();
        assert_eq!(urc.command(), "URC");
        assert_eq!(urc.values().as_slice(), ["1", "string"]);
        assert_eq!(MAIN_RX_CHANNEL.try_receive().unwrap()?, FromModem::Error);

        let long = "123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890X";
        block_on(broker.parse_lines(long));
        assert_eq!(
            MAIN_RX_CHANNEL.try_receive().unwrap(),
            Err(Error::BufferTooSmallError)
        );

        block_on(broker.parse_lines("+NONURC: 1\n"));
        assert_eq!(
            MAIN_RX_CHANNEL.try_receive().unwrap()?,
            FromModem::CommandResponse(CommandResponse::new("+NONURC: 1")?)
        );
        assert_eq!(MAIN_RX_CHANNEL.try_receive().unwrap()?, FromModem::Eof);
        assert_eq!(MAIN_RX_CHANNEL.len(), 0);
        Ok(())
    }
}
