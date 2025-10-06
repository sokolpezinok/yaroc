//! AT-command based UART communication.
//!
//! This module provides a generic implementation for AT-command based communication. The physical
//! layer is abstracted away by the `RxWithIdle` and `Tx` traits. The module provides a broker that
//! parses all incoming bytes and routes them to either a URC handler or the main channel for
//! command-specific replies.

use super::response::{AT_COMMAND_SIZE, AT_LINES, AtResponse, CommandResponse, FromModem};
use core::str::FromStr;
#[cfg(feature = "defmt")]
use defmt::{self, debug};
use embassy_executor::Spawner;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, WithTimeout};
use heapless::{String, Vec, format};
#[cfg(not(feature = "defmt"))]
use log::debug;

use crate::{RawMutex, error::Error};

/// A channel for receiving AT-command replies from the modem.
pub type MainRxChannelType = Channel<RawMutex, Result<FromModem, Error>, 5>;
/// A handler for Unsolicited Result Codes (URCs).
pub type UrcHandlerType = fn(&CommandResponse) -> bool;
/// The main channel for receiving AT-command replies from the modem.
pub static MAIN_RX_CHANNEL: MainRxChannelType = Channel::new();

/// A broker of AT replies (listening to UART RX) and routing each reply either to the main channel
/// or channel dedicated to URCs.
pub struct AtRxBroker {
    main_channel: &'static MainRxChannelType,
    urc_handler: UrcHandlerType,
}

impl AtRxBroker {
    /// Creates a new `AtRxBroker`.
    ///
    /// # Arguments
    /// * `main_channel` - The channel for routing non-URC replies.
    /// * `urc_handler` - A function for handling URCs.
    pub fn new(main_channel: &'static MainRxChannelType, urc_handler: UrcHandlerType) -> Self {
        Self {
            main_channel,
            urc_handler,
        }
    }

    /// Parses lines out of a given text and forwards each line to the appropriate channel.
    ///
    /// # Arguments
    /// * `text` - The text to be parsed.
    async fn parse_lines(&self, text: &str) {
        let lines = text.lines().filter(|line| !line.is_empty());
        let mut open_stream = false;
        for line in lines {
            let to_send = match line {
                "OK" | "RDY" | "APP RDY" | "> " => Ok(FromModem::Ok),
                "ERROR" => Ok(FromModem::Error),
                line => match CommandResponse::new(line) {
                    Ok(command_response) => Ok(FromModem::CommandResponse(command_response)),
                    _ => String::from_str(line)
                        .map(FromModem::Line)
                        .map_err(|_| Error::BufferTooSmallError),
                },
            };

            if let Ok(FromModem::CommandResponse(command_response)) = to_send.as_ref()
                && (self.urc_handler)(command_response)
            {
                #[cfg(feature = "defmt")]
                debug!("Got URC {}", line);
                continue;
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

    /// Runs the AT-command broker loop.
    ///
    /// This function reads all bytes from `rx` until it's idle and parses the bytes into lines
    /// ('\n' or '\r\n' are both accepted).
    ///
    /// This function is intended to be run as a background task.
    ///
    /// Note that if 300 characters are read at once, the last line will be cut in the middle. This
    /// might be fixed in the future.
    ///
    /// # Arguments
    /// * `rx` - The UART receiver to read from.
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

/// A trait for reading from a UART that can detect when the line is idle.
pub trait RxWithIdle {
    /// Spawns a new task to handle incoming UART data.
    ///
    /// # Arguments
    /// * `spawner` - The task spawner.
    /// * `urc_handler` - A function for handling URCs.
    fn spawn(self, spawner: Spawner, urc_handler: UrcHandlerType);

    /// Reads from the UART until the line is idle.
    ///
    /// # Arguments
    /// * `buf` - The buffer to read bytes into.
    ///
    /// # Returns
    /// The number of bytes read.
    fn read_until_idle(
        &mut self,
        buf: &mut [u8],
    ) -> impl core::future::Future<Output = crate::Result<usize>>;
}

/// A trait for writing to a UART.
pub trait Tx {
    /// Writes bytes to the UART.
    ///
    /// # Arguments
    /// * `buffer` - The buffer to write to the UART.
    fn write(&mut self, buffer: &[u8]) -> impl core::future::Future<Output = crate::Result<()>>;
}

/// A channel for sending AT-commands to the modem.
pub type TxChannelType = Channel<RawMutex, String<AT_COMMAND_SIZE>, 5>;

/// Fake RxWithIdle, to be used in tests.
pub struct FakeRxWithIdle {
    responses: Vec<(&'static str, &'static str), 10>,
    tx_channel: &'static TxChannelType,
}

impl FakeRxWithIdle {
    /// Creates a new `FakeRxWithIdle`.
    ///
    /// # Arguments
    /// * `responses` - A list of expected commands and their responses.
    /// * `tx_channel` - The channel for transmitting AT-commands.
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
    fn spawn(self, spawner: Spawner, urc_handler: UrcHandlerType) {
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

impl Tx for &'static TxChannelType {
    async fn write(&mut self, buffer: &[u8]) -> crate::Result<()> {
        let s = core::str::from_utf8(buffer).map_err(|_| Error::StringEncodingError)?;
        let s = String::from_str(s).map_err(|_| Error::BufferTooSmallError)?;
        self.send(s).await;
        Ok(())
    }
}

/// A UART for sending and receiving AT-commands.
///
/// The TX part is represented by the `Tx` trait, and the RX part is represented by the
/// `RxWithIdle` trait.
pub struct AtUart<T: Tx, R: RxWithIdle> {
    tx: T,
    rx: Option<R>,
    main_rx_channel: &'static MainRxChannelType,
}

impl<T: Tx, R: RxWithIdle> AtUart<T, R> {
    /// Creates a new `AtUart`.
    ///
    /// # Arguments
    /// * `tx` - The UART transmitter.
    /// * `rx` - The UART receiver.
    pub fn new(tx: T, rx: R) -> Self {
        Self {
            tx,
            rx: Some(rx),
            main_rx_channel: &MAIN_RX_CHANNEL,
        }
    }

    /// Spawns a task that reads from the UART and brokers the replies.
    ///
    /// # Arguments
    /// * `urc_handler` - A function for handling URCs.
    /// * `spawner` - The task spawner.
    pub fn spawn_rx(&mut self, urc_handler: UrcHandlerType, spawner: Spawner) {
        // Consume self.rx, then set self.rx = None
        let rx = self.rx.take();
        rx.unwrap().spawn(spawner, urc_handler);
    }

    /// Reads a reply from the modem.
    ///
    /// # Arguments
    /// * `timeout` - The maximum time to wait for a reply.
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

    /// Writes an AT command to the modem.
    ///
    /// # Arguments
    /// * `command` - The AT command to write.
    async fn write_at(&mut self, command: &str) -> Result<(), Error> {
        let command = format!(AT_COMMAND_SIZE; "AT{command}\r")?;
        self.write(command.as_bytes()).await
    }

    /// Writes a raw message to the modem.
    ///
    /// # Arguments
    /// * `message` - The message to write.
    async fn write(&mut self, message: &[u8]) -> crate::Result<()> {
        self.tx.write(message).await.map_err(|_| Error::UartWriteError)
    }

    /// Calls an AT command and waits for a reply.
    ///
    /// # Arguments
    /// * `msg` - The raw message to send.
    /// * `command_prefix` - The prefix of the command being sent.
    /// * `second_read` - Whether to perform a second read.
    /// * `timeout` - The timeout for each read.
    pub async fn call(
        &mut self,
        msg: &[u8],
        command_prefix: &str,
        second_read: bool,
        timeout: Duration,
    ) -> crate::Result<AtResponse> {
        let start = Instant::now();
        self.write(msg).await?;
        // This is used for +QMTPUB, we have to read twice, because there's a pause. As a
        // technicality, the timeout is doubled this way, but it's never a problem.
        let mut lines = self.read(timeout).await?;
        if second_read {
            lines.extend(self.read(timeout).await?);
        }
        let response = AtResponse::new(lines, command_prefix);
        debug!(
            "{}: {}, took {}ms",
            command_prefix,
            response,
            (Instant::now() - start).as_millis()
        );
        Ok(response)
    }

    /// Calls an AT command and waits for a reply, with a final `OK` or `ERROR`.
    ///
    /// # Arguments
    /// * `command` - The AT command to call.
    /// * `timeout` - The timeout for the read.
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

    /// Calls an AT command and waits for a reply, with an optional second read for URCs.
    ///
    /// # Arguments
    /// * `command` - The AT command to call.
    /// * `call_timeout` - The timeout for the initial command call.
    /// * `response_timeout` - An optional timeout for a second read to catch URCs.
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
        let handler = |response: &CommandResponse| match response.command() {
            "URC" => {
                URC_CHANNEL.try_send(response.clone()).unwrap();
                true
            }
            _ => false,
        };
        let broker = AtRxBroker::new(&MAIN_RX_CHANNEL, handler);

        block_on(broker.parse_lines("OK\n+URC: 1,\"string\"\nERROR"));
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
