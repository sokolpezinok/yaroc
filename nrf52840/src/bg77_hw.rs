use defmt::info;
use embassy_executor::Spawner;
use embassy_nrf::gpio::Output;
use embassy_time::{Duration, Timer};
use yaroc_common::at::{
    response::AtResponse,
    uart::{AtUart, RxWithIdle, Tx, UrcHandlerType},
};

static BG77_MINIMUM_TIMEOUT: Duration = Duration::from_millis(300);

pub trait ModemPin {
    fn set_low(&mut self);
    fn set_high(&mut self);
}

impl ModemPin for Output<'static> {
    fn set_low(&mut self) {
        self.set_low();
    }

    fn set_high(&mut self) {
        self.set_high();
    }
}

/// Struct for accessing Quectel BG77 modem
pub struct Bg77<T: Tx, R: RxWithIdle, P: ModemPin> {
    uart1: AtUart<T, R>,
    modem_pin: P,
}

pub trait ModemHw {
    /// Spawn a task for the modem and process incoming URCs using the provided handler.
    fn spawn(&mut self, urc_handler: UrcHandlerType, spawner: Spawner);

    /// Performs an AT call to the modem, optionally also waiting longer for a response.
    ///
    /// The command send is `cmd` prefixed with `AT`. We wait a short time for an OK/ERROR and then
    /// if `response_timeout` is set, we wait `response_timeout` for a response that is prefixed by
    /// `cmd`.
    fn simple_call_at(
        &mut self,
        cmd: &str,
        response_timeout: Option<Duration>,
    ) -> impl core::future::Future<Output = crate::Result<AtResponse>>;

    /// Performs an AT call to the modem and waits for an OK/ERROR response.
    ///
    /// The maximum waiting time is specified by `timeout`.
    fn call_at(
        &mut self,
        cmd: &str,
        timeout: Duration,
    ) -> impl core::future::Future<Output = crate::Result<AtResponse>>;

    /// Sends a raw message to the modem.
    ///
    /// Waits only a short time for a response (non-configurable). The response should be prefixed
    /// with `command_prefix`.
    fn call(
        &mut self,
        msg: &[u8],
        command_prefix: &str,
        second_read: bool,
    ) -> impl core::future::Future<Output = crate::Result<AtResponse>>;

    /// TODO: docstring
    fn read(&mut self) -> impl core::future::Future<Output = crate::Result<AtResponse>>;

    /// Turns on the modem.
    fn turn_on(&mut self) -> impl core::future::Future<Output = crate::Result<()>>;
}

impl<T: Tx, R: RxWithIdle, P: ModemPin> Bg77<T, R, P> {
    pub fn new(tx: T, rx: R, modem_pin: P) -> Self {
        let uart1 = AtUart::new(tx, rx);
        Self { uart1, modem_pin }
    }
}

impl<T: Tx, R: RxWithIdle, P: ModemPin> ModemHw for Bg77<T, R, P> {
    fn spawn(&mut self, urc_handler: UrcHandlerType, spawner: Spawner) {
        self.uart1.spawn_rx(urc_handler, spawner);
    }

    async fn simple_call_at(
        &mut self,
        cmd: &str,
        response_timeout: Option<Duration>,
    ) -> crate::Result<AtResponse> {
        self.uart1.call_at(cmd, BG77_MINIMUM_TIMEOUT, response_timeout).await
    }

    async fn call_at(&mut self, cmd: &str, timeout: Duration) -> yaroc_common::Result<AtResponse> {
        self.uart1.call_at(cmd, timeout, None).await
    }

    async fn call(
        &mut self,
        msg: &[u8],
        command_prefix: &str,
        second_read: bool,
    ) -> yaroc_common::Result<AtResponse> {
        self.uart1.call(msg, command_prefix, second_read, BG77_MINIMUM_TIMEOUT).await
    }

    async fn read(&mut self) -> crate::Result<AtResponse> {
        let lines = self.uart1.read(BG77_MINIMUM_TIMEOUT).await?;
        // TODO: take command as parameter
        Ok(AtResponse::new(lines, "+QMTPUB"))
    }

    async fn turn_on(&mut self) -> crate::Result<()> {
        if self.simple_call_at("", None).await.is_err() {
            self.modem_pin.set_low();
            Timer::after_secs(1).await;
            self.modem_pin.set_high();
            Timer::after_secs(2).await;
            self.modem_pin.set_low();
            let res = self.uart1.read(Duration::from_secs(5)).await?;
            info!("Modem response: {=[?]}", res.as_slice());
            self.call_at("+CFUN=1,0", Duration::from_secs(15)).await?;
            let res = self.uart1.read(Duration::from_secs(5)).await?;
            info!("Modem response: {=[?]}", res.as_slice());
        }
        Ok(())
    }
}
