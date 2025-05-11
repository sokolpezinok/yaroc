use core::str::FromStr;

use defmt::info;
use embassy_executor::Spawner;
use embassy_nrf::gpio::Output;
use embassy_time::{Duration, Timer};
use heapless::{format, String};
use yaroc_common::{
    at::{
        response::AtResponse,
        uart::{AtUart, RxWithIdle, Tx, UrcHandlerType},
    },
    error::Error,
};

static BG77_MINIMUM_TIMEOUT: Duration = Duration::from_millis(300);
pub static ACTIVATION_TIMEOUT: Duration = Duration::from_secs(150);

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

pub struct FakePin {}
impl ModemPin for FakePin {
    fn set_low(&mut self) {}
    fn set_high(&mut self) {}
}

pub enum RAT {
    Ltem,      // LTE-M
    NbIot,     // NB-IoT
    LtemNbIot, // Both
}

pub struct ModemConfig {
    pub apn: String<30>,
    pub rat: RAT,
}

impl Default for ModemConfig {
    fn default() -> Self {
        Self {
            apn: String::from_str("internet.iot").unwrap(),
            rat: RAT::LtemNbIot,
        }
    }
}

/// Struct for accessing Quectel BG77 modem
pub struct Bg77<T: Tx, R: RxWithIdle, P: ModemPin> {
    uart1: AtUart<T, R>,
    modem_pin: P,
    config: ModemConfig,
}

pub trait ModemHw {
    /// Configures the modem according to a modem config.
    fn configure(&mut self) -> impl core::future::Future<Output = Result<(), Error>>;

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
    /// Waits for a response if `second_read_timeout` is set and the timeout is the value of
    /// `second_read_timeout`. The response should be prefixed with `command_prefix`.
    fn call(
        &mut self,
        msg: &[u8],
        command_prefix: &str,
        second_read_timeout: Option<Duration>,
    ) -> impl core::future::Future<Output = crate::Result<AtResponse>>;

    /// TODO: docstring
    fn read(&mut self) -> impl core::future::Future<Output = crate::Result<AtResponse>>;

    /// Turns on the modem.
    fn turn_on(&mut self) -> impl core::future::Future<Output = crate::Result<()>>;
}

impl<T: Tx, R: RxWithIdle, P: ModemPin> Bg77<T, R, P> {
    pub fn new(tx: T, rx: R, modem_pin: P, config: ModemConfig) -> Self {
        let uart1 = AtUart::new(tx, rx);
        Self {
            uart1,
            modem_pin,
            config,
        }
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
        second_read_timeout: Option<Duration>,
    ) -> yaroc_common::Result<AtResponse> {
        match second_read_timeout {
            None => self.uart1.call(msg, command_prefix, false, BG77_MINIMUM_TIMEOUT).await,
            Some(timeout) => self.uart1.call(msg, command_prefix, true, timeout).await,
        }
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

    async fn configure(&mut self) -> Result<(), Error> {
        self.simple_call_at("E0", None).await?;
        let cmd = format!(100; "+CGDCONT=1,\"IP\",\"{}\"", self.config.apn)?;
        let _ = self.simple_call_at(&cmd, None).await;
        self.simple_call_at("+CEREG=2", None).await?;
        let _ = self.call_at("+CGATT=1", ACTIVATION_TIMEOUT).await;

        let (nwscanseq, iotopmode) = match self.config.rat {
            RAT::Ltem => ("02", 0),
            RAT::NbIot => ("03", 1),
            RAT::LtemNbIot => ("00", 2),
        };
        let cmd = format!(50; "+QCFG=\"nwscanseq\",{}", nwscanseq)?;
        self.simple_call_at(&cmd, None).await?;
        let cmd = format!(50; "+QCFG=\"iotopmode\",{},1", iotopmode)?;
        self.simple_call_at(&cmd, None).await?;
        //TODO: allow band configuration
        self.simple_call_at("+QCFG=\"band\",0,80000,80000", None).await?;
        Ok(())
    }
}
