use core::str::FromStr;

use crate::at::{
    response::AtResponse,
    uart::{AtUart, RxWithIdle, Tx, UrcHandlerType},
};
use crate::error::Error;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use heapless::{String, format};

/// Minimum timeout for BG77 AT-command responses.
static BG77_MINIMUM_TIMEOUT: Duration = Duration::from_millis(300);
/// Timeout for network activation.
pub static ACTIVATION_TIMEOUT: Duration = Duration::from_secs(150);

/// PIN for turning on the modem
pub trait ModemPin {
    fn set_low(&mut self);
    fn set_high(&mut self);
}

#[cfg(feature = "nrf")]
impl ModemPin for embassy_nrf::gpio::Output<'static> {
    fn set_low(&mut self) {
        self.set_low();
    }

    fn set_high(&mut self) {
        self.set_high();
    }
}

/// A fake modem pin for testing purposes, it does nothing.
pub struct FakePin {}
impl ModemPin for FakePin {
    fn set_low(&mut self) {}
    fn set_high(&mut self) {}
}

/// Radio Access Technology
pub enum RAT {
    Ltem,      // LTE-M
    NbIot,     // NB-IoT
    LtemNbIot, // Both
}

#[derive(Default, Clone, Copy)]
pub struct LteBands {
    /// LTE-M bands bitmask. Bit `n` corresponds to band `n+1`.
    pub ltem: u128,
    /// NB-IoT bands bitmask. Bit `n` corresponds to band `n+1`.
    pub nbiot: u128,
}

impl LteBands {
    /// Sets the LTE-M bands from a slice of band numbers.
    ///
    /// This will overwrite any previously set LTE-M bands.
    /// Bands should be given as numbers, e.g., 20 for B20.
    /// Invalid band numbers (0 or > 128) are ignored.
    pub fn set_ltem_bands(&mut self, bands: &[u32]) {
        self.ltem = 0;
        for &band in bands {
            if band > 0 && band <= 128 {
                self.ltem |= 1_u128 << (band - 1);
            }
        }
    }

    /// Sets the NB-IoT bands from a slice of band numbers.
    ///
    /// This will overwrite any previously set NB-IoT bands.
    /// Bands should be given as numbers, e.g., 20 for B20.
    /// Invalid band numbers (0 or > 128) are ignored.
    pub fn set_nbiot_bands(&mut self, bands: &[u32]) {
        self.nbiot = 0;
        for &band in bands {
            if band > 0 && band <= 128 {
                self.nbiot |= 1_u128 << (band - 1);
            }
        }
    }
}

pub struct ModemConfig {
    /// Access point name (APN)
    pub apn: String<30>,
    /// Radio access technology (RAT)
    pub rat: RAT,
    /// LTE bands
    pub bands: LteBands,
}

impl Default for ModemConfig {
    /// Creates a default modem configuration.
    fn default() -> Self {
        let mut bands = LteBands::default();
        // Default bands are B20 for both LTE-M and NB-IoT
        bands.set_ltem_bands(&[20]);
        bands.set_nbiot_bands(&[20]);
        Self {
            apn: String::from_str("internet.iot").unwrap(),
            rat: RAT::LtemNbIot,
            bands,
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

    /// Reads an AT response from the modem.
    fn read(&mut self) -> impl core::future::Future<Output = crate::Result<AtResponse>>;

    /// Turns on the modem.
    fn turn_on(&mut self) -> impl core::future::Future<Output = crate::Result<()>>;
}

impl<T: Tx, R: RxWithIdle, P: ModemPin> Bg77<T, R, P> {
    /// Creates a new `Bg77` modem instance.
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

    async fn call_at(&mut self, cmd: &str, timeout: Duration) -> crate::Result<AtResponse> {
        self.uart1.call_at(cmd, timeout, None).await
    }

    async fn call(
        &mut self,
        msg: &[u8],
        command_prefix: &str,
        second_read_timeout: Option<Duration>,
    ) -> crate::Result<AtResponse> {
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
            let _res = self.uart1.read(Duration::from_secs(5)).await?;
            #[cfg(feature = "defmt")]
            defmt::info!("Modem response: {=[?]}", _res.as_slice());
            self.call_at("+CFUN=1,0", Duration::from_secs(15)).await?;
            let _res = self.uart1.read(Duration::from_secs(5)).await?;
            #[cfg(feature = "defmt")]
            defmt::info!("Modem response: {=[?]}", _res.as_slice());
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
        let cmd = format!(100; "+QCFG=\"band\",0,{:x},{:x}", self.config.bands.ltem, self.config.bands.nbiot)?;
        self.simple_call_at(&cmd, None).await?;
        Ok(())
    }
}
