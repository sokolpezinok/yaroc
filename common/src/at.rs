use core::option::Option::Some;
use core::str::FromStr;
#[cfg(feature = "defmt")]
use defmt::{self, debug};
use embassy_executor::Spawner;
use embassy_sync::channel::Channel;
use embassy_time::{with_deadline, Duration, Instant};
use heapless::{format, String, Vec};
#[cfg(not(feature = "defmt"))]
use log::debug;
use response::{split_at_response, AtResponse, FromModem, AT_COMMAND_SIZE, AT_LINES};

use crate::error::Error;

pub mod response;

static MAIN_CHANNEL: MainChannelType<Error> = Channel::new();

#[cfg(all(target_abi = "eabihf", target_os = "none"))]
type RawMutex = embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
#[cfg(not(all(target_abi = "eabihf", target_os = "none")))]
type RawMutex = embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

pub type MainChannelType<E> = Channel<RawMutex, Result<FromModem, E>, 5>;
pub type UrcChannelType = Channel<RawMutex, Result<String<AT_COMMAND_SIZE>, Error>, 2>;

pub struct AtBroker<E: 'static + From<Error>> {
    main_channel: &'static MainChannelType<E>,
    urc_channel: &'static UrcChannelType,
}

impl<E: From<Error>> AtBroker<E> {
    pub fn new(
        main_channel: &'static MainChannelType<E>,
        urc_channel: &'static UrcChannelType,
    ) -> Self {
        Self {
            main_channel,
            urc_channel,
        }
    }

    pub async fn parse_lines(&self, text: &str, urc_handler: fn(&str, &str) -> bool) {
        let lines = text.lines().filter(|line| !line.is_empty());
        let mut open_stream = false;
        for line in lines {
            let is_callback = split_at_response(line)
                .map(|(prefix, rest)| (urc_handler)(prefix, rest))
                .unwrap_or_default();

            let to_send = match line {
                "OK" | "RDY" | "APP RDY" => Ok(FromModem::Ok),
                "ERROR" => Ok(FromModem::Error),
                line => String::from_str(line)
                    .map(FromModem::Line)
                    .map_err(|_| Error::BufferTooSmallError.into()),
            };
            if !is_callback {
                if let Ok(FromModem::Line(_)) = to_send.as_ref() {
                    open_stream = true;
                } else {
                    open_stream = false;
                }
                self.main_channel.send(to_send).await;
            } else {
                self.urc_channel
                    .send(String::from_str(line).map_err(|_| Error::BufferTooSmallError))
                    .await;
            }
        }
        if open_stream {
            self.main_channel.send(Ok(FromModem::Ok)).await; // Stop transmission
        }
    }
}

pub trait RxWithIdle {
    /// Spawn a new task on `spawner` that reads RX from UART and clasifies answers using
    /// `urc_classifier`.
    fn spawn(
        self,
        spawner: &Spawner,
        urc_classifier: fn(&str, &str) -> bool,
        main_channel: &'static MainChannelType<Error>,
    );
}

pub trait Tx {
    fn write(&mut self, buffer: &[u8]) -> impl core::future::Future<Output = crate::Result<()>>;
}

pub struct AtUart<T: Tx> {
    tx: T,
    main_channel: &'static MainChannelType<Error>,
}

impl<T: Tx> AtUart<T> {
    pub fn new<R: RxWithIdle>(
        rx: R,
        tx: T,
        urc_classifier: fn(&str, &str) -> bool,
        spawner: &Spawner,
    ) -> Self {
        rx.spawn(spawner, urc_classifier, &MAIN_CHANNEL);
        Self {
            tx,
            main_channel: &MAIN_CHANNEL,
        }
    }

    pub async fn read(&self, timeout: Duration) -> Result<Vec<FromModem, 4>, Error> {
        let mut res = Vec::new();
        let deadline = Instant::now() + timeout;
        loop {
            let from_modem = with_deadline(deadline, self.main_channel.receive())
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
                // TODO: fix for no defmt
                //debug!(
                //    "Failed response from modem: {} {=[?]}",
                //    command,
                //    lines.as_slice()
                //);
                Err(Error::AtErrorResponse)
            }
            _ => {
                // TODO: fix for no defmt
                //debug!(
                //    "Failed response from modem: {} {=[?]}",
                //    command,
                //    lines.as_slice()
                //);
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
    fn test_at_broker() {
        static MAIN_CHANNEL: MainChannelType<Error> = Channel::new();
        static URC_CHANNEL: UrcChannelType = Channel::new();
        let broker = AtBroker::new(&MAIN_CHANNEL, &URC_CHANNEL);
        let handler = |prefix: &str, _: &str| match prefix {
            "URC" => true,
            _ => false,
        };

        block_on(broker.parse_lines("OK\r\n+URC: 1\nERROR", handler));
        assert_eq!(block_on(MAIN_CHANNEL.receive()).unwrap(), FromModem::Ok);
        assert_eq!(block_on(URC_CHANNEL.receive()).unwrap(), "+URC: 1");
        assert_eq!(block_on(MAIN_CHANNEL.receive()).unwrap(), FromModem::Error);

        let long = "123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890X";
        block_on(broker.parse_lines(long, handler));
        assert_eq!(
            MAIN_CHANNEL.try_receive().unwrap(),
            Err(Error::BufferTooSmallError)
        );

        block_on(broker.parse_lines("+NONURC: 1\n", handler));
        assert_eq!(
            block_on(MAIN_CHANNEL.receive()).unwrap(),
            FromModem::Line(String::from_str("+NONURC: 1").unwrap())
        );
        assert_eq!(block_on(MAIN_CHANNEL.receive()).unwrap(), FromModem::Ok);
        assert_eq!(MAIN_CHANNEL.len(), 0);
    }
}
