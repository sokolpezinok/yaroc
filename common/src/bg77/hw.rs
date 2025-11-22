use core::str::FromStr;

#[cfg(feature = "nrf")]
use crate::at::uart::{AtUart, AtUartTrait, RxWithIdle, Tx};
use crate::at::{
    response::{AT_COMMAND_SIZE, AtResponse, CommandResponse, FromModem},
    uart::UrcHandlerType,
};
use embassy_executor::Spawner;
#[cfg(feature = "nrf")]
use embassy_nrf::gpio::Output;
use embassy_time::Duration;
use heapless::{String, format, index_map::FnvIndexMap};

#[cfg(feature = "nrf")]
/// Minimum timeout for BG77 AT-command responses.
static BG77_MINIMUM_TIMEOUT: Duration = Duration::from_millis(300);

pub trait ModemHw {
    /// Default timeout for a command. It's typically the minimum timeout, a couple hundred
    /// milliseconds for a modem.
    const DEFAULT_TIMEOUT: Duration;

    /// Spawn a task for the modem and process incoming URCs using the provided handlers.
    fn spawn(&mut self, spawner: Spawner, urc_handlers: &[UrcHandlerType]);

    /// Performs an AT call to the modem, optionally also waiting longer for a response.
    ///
    /// We send `cmd` prefixed by `AT`. We wait a short time for an OK/ERROR and then if
    /// `response_timeout` is set, we wait `response_timeout` for a response that is prefixed by
    /// `cmd`.
    fn call_at(
        &mut self,
        cmd: &str,
        response_timeout: Option<Duration>,
    ) -> impl core::future::Future<Output = crate::Result<AtResponse>>;

    /// Performs an AT call to the modem and waits for an OK/ERROR response.
    ///
    /// The maximum waiting time is specified by `timeout`.
    fn long_call_at(
        &mut self,
        cmd: &str,
        timeout: Duration,
    ) -> impl core::future::Future<Output = crate::Result<AtResponse>>;

    /// Sends a raw message to the modem.
    ///
    /// Waits for a response if `second_read_timeout` is set and the timeout is the value of
    /// `second_read_timeout`. The response should be prefixed with `command_prefix`.
    fn call(
        &mut self,
        msg: &[u8],
        command_prefix: &str,
        second_read_timeout: Option<Duration>,
    ) -> impl core::future::Future<Output = crate::Result<AtResponse>>;

    /// Reads an AT response from the modem.
    fn read(&mut self) -> impl core::future::Future<Output = crate::Result<AtResponse>>;

    /// Turns on the modem.
    fn turn_on(&mut self) -> impl core::future::Future<Output = crate::Result<()>>;
}

pub struct FakeModem {
    responses: FnvIndexMap<String<AT_COMMAND_SIZE>, String<60>, 8>,
}

impl FakeModem {
    pub fn new(interactions: &[(&str, &str)]) -> Self {
        let mut responses = FnvIndexMap::new();
        for (command, response) in interactions {
            responses
                .insert(
                    String::from_str(command).unwrap(),
                    String::from_str(response).unwrap(),
                )
                .unwrap();
        }
        Self { responses }
    }
}

//TODO: might be better to use a mocking library here
impl ModemHw for FakeModem {
    const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1);

    fn spawn(&mut self, _spawner: Spawner, _urc_handlers: &[UrcHandlerType]) {}

    async fn call_at(
        &mut self,
        cmd: &str,
        _response_timeout: Option<Duration>,
    ) -> crate::Result<AtResponse> {
        let at_cmd = format!(AT_COMMAND_SIZE; "AT{cmd}").unwrap();
        let command_response =
            CommandResponse::new(self.responses.get(at_cmd.as_str()).unwrap()).unwrap();
        let response = FromModem::CommandResponse(command_response);
        Ok(AtResponse::new([response, FromModem::Ok].into(), cmd))
    }

    async fn long_call_at(&mut self, _cmd: &str, _timeout: Duration) -> crate::Result<AtResponse> {
        todo!();
    }

    async fn call(
        &mut self,
        _msg: &[u8],
        _command_prefix: &str,
        _second_read_timeout: Option<Duration>,
    ) -> crate::Result<AtResponse> {
        todo!()
    }

    async fn read(&mut self) -> crate::Result<AtResponse> {
        todo!()
    }

    async fn turn_on(&mut self) -> crate::Result<()> {
        todo!()
    }
}

/// Struct for accessing Quectel BG77 modem
#[cfg(feature = "nrf")]
pub struct Bg77<T: Tx, R: RxWithIdle> {
    uart1: AtUart<T, R>,
    modem_pin: Output<'static>,
}

#[cfg(feature = "nrf")]
impl<T: Tx, R: RxWithIdle> Bg77<T, R> {
    /// Creates a new `Bg77` modem instance.
    pub fn new(tx: T, rx: R, modem_pin: Output<'static>) -> Self {
        let uart1 = AtUart::new(tx, rx);
        Self { uart1, modem_pin }
    }
}

#[cfg(feature = "nrf")]
impl<T: Tx, R: RxWithIdle> ModemHw for Bg77<T, R> {
    const DEFAULT_TIMEOUT: Duration = BG77_MINIMUM_TIMEOUT;

    fn spawn(&mut self, spawner: Spawner, urc_handlers: &[UrcHandlerType]) {
        self.uart1.spawn_rx(urc_handlers, spawner);
    }

    async fn call_at(
        &mut self,
        cmd: &str,
        response_timeout: Option<Duration>,
    ) -> crate::Result<AtResponse> {
        self.uart1.call_at(cmd, Self::DEFAULT_TIMEOUT, response_timeout).await
    }

    async fn long_call_at(&mut self, cmd: &str, timeout: Duration) -> crate::Result<AtResponse> {
        self.uart1.call_at(cmd, timeout, None).await
    }

    async fn call(
        &mut self,
        msg: &[u8],
        command_prefix: &str,
        second_read_timeout: Option<Duration>,
    ) -> crate::Result<AtResponse> {
        match second_read_timeout {
            None => self.uart1.call(msg, command_prefix, false, Self::DEFAULT_TIMEOUT).await,
            Some(timeout) => self.uart1.call(msg, command_prefix, true, timeout).await,
        }
    }

    async fn read(&mut self) -> crate::Result<AtResponse> {
        let lines = self.uart1.read(Self::DEFAULT_TIMEOUT).await?;
        // TODO: take command as parameter
        Ok(AtResponse::new(lines, "+QMTPUB"))
    }

    async fn turn_on(&mut self) -> crate::Result<()> {
        if self.call_at("", None).await.is_err() {
            self.modem_pin.set_low();
            embassy_time::Timer::after_secs(1).await;
            self.modem_pin.set_high();
            embassy_time::Timer::after_secs(2).await;
            self.modem_pin.set_low();
            let _res = self.uart1.read(Duration::from_secs(5)).await?;
            #[cfg(feature = "defmt")]
            defmt::info!("Modem response: {=[?]}", _res.as_slice());
            self.long_call_at("+CFUN=1,0", Duration::from_secs(15)).await?;
            let _res = self.uart1.read(Duration::from_secs(5)).await?;
            #[cfg(feature = "defmt")]
            defmt::info!("Modem response: {=[?]}", _res.as_slice());
        }
        Ok(())
    }
}
