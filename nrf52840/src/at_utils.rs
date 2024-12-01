use common::at::{
    AtBroker, AtResponse, FromModem, MainChannelType, RxWithIdle, Tx, AT_COMMAND_SIZE, AT_LINES,
};
use core::str::from_utf8;
use defmt::*;
use embassy_executor::Spawner;
use embassy_nrf::peripherals::{TIMER0, UARTE1};
use embassy_nrf::uarte::{UarteRxWithIdle as EmbassyUarteRxWithIdle, UarteTx as EmbassyUarteTx};
use embassy_sync::channel::Channel;
use embassy_time::{with_deadline, Duration, Instant};
use heapless::{format, Vec};

use crate::error::Error;

static MAIN_CHANNEL: MainChannelType<common::error::Error> = Channel::new();
pub static URC_CHANNEL: common::at::UrcChannelType = Channel::new();

/// RX reader task implemented for UarteRxWithIdle.
#[embassy_executor::task]
async fn reader(
    mut rx: EmbassyUarteRxWithIdle<'static, UARTE1, TIMER0>,
    urc_classifier: fn(&str, &str) -> bool,
    main_channel: &'static MainChannelType<common::error::Error>,
) {
    const AT_BUF_SIZE: usize = 300;
    let mut buf = [0; AT_BUF_SIZE];
    let at_broker = AtBroker::new(main_channel, &URC_CHANNEL);
    loop {
        let len = rx
            .read_until_idle(&mut buf)
            .await
            .map_err(|_| common::error::Error::UartReadError);
        match len {
            Err(err) => main_channel.send(Err(err)).await,
            Ok(len) => {
                let text = from_utf8(&buf[..len]);
                match text {
                    Err(_) => {
                        main_channel.send(Err(common::error::Error::StringEncodingError)).await
                    }
                    Ok(text) => at_broker.parse_lines(text, urc_classifier).await,
                }
            }
        }
    }
}

pub struct UarteRxWithIdle {
    // This struct is fixed to UARTE1 due to a limitation of embassy_executor::task. We cannot make
    // the `reader` method generic and also work for UARTE0. However, for our hardware this is not
    // needed, UARTE0 does not use AT-commands, so it won't use this struct.
    rx: EmbassyUarteRxWithIdle<'static, UARTE1, TIMER0>,
}

impl UarteRxWithIdle {
    pub fn new(rx: EmbassyUarteRxWithIdle<'static, UARTE1, TIMER0>) -> Self {
        Self { rx }
    }
}

impl RxWithIdle for UarteRxWithIdle {
    fn spawn(
        self,
        spawner: &Spawner,
        urc_classifier: fn(&str, &str) -> bool,
        main_channel: &'static MainChannelType<common::error::Error>,
    ) {
        unwrap!(spawner.spawn(reader(self.rx, urc_classifier, main_channel)));
    }
}

pub struct UarteTx {
    // This struct is fixed to UARTE1 due to a limitation of embassy_executor::task. We cannot make
    // the `reader` method generic and also work for UARTE0. However, for our hardware this is not
    // needed, UARTE0 does not use AT-commands, so it won't use this struct.
    tx: EmbassyUarteTx<'static, UARTE1>,
}

impl UarteTx {
    pub fn new(tx: EmbassyUarteTx<'static, UARTE1>) -> Self {
        Self { tx }
    }
}

impl Tx for UarteTx {
    async fn write(&mut self, buffer: &[u8]) -> common::Result<()> {
        self.tx.write(buffer).await.map_err(|_| common::error::Error::UartWriteError)
    }
}

pub struct AtUart<T: Tx> {
    tx: T,
    main_channel: &'static MainChannelType<common::error::Error>,
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
