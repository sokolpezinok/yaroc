use core::future::Future;

#[cfg(feature = "defmt")]
use defmt::debug;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use heapless::Vec;
#[cfg(not(feature = "defmt"))]
use log::debug;

use crate::at::AtError;
use crate::at::response::{AT_LINES, AtResponse, FromModem};
use crate::at::uart::{AtUartTrait, UrcHandlerType};
use crate::error::Error;

#[cfg(feature = "nrf")]
use embassy_nrf::gpio::Output;

/// Trait for controlling the modem power pin.
pub trait ModemPin {
    /// Sets the pin output to high.
    fn set_high(&mut self);
    /// Sets the pin output to low.
    fn set_low(&mut self);
}

pub struct FakePin {}

impl ModemPin for FakePin {
    fn set_high(&mut self) {}
    fn set_low(&mut self) {}
}

#[cfg(feature = "nrf")]
impl ModemPin for Output<'static> {
    fn set_high(&mut self) {
        self.set_high();
    }

    fn set_low(&mut self) {
        self.set_low();
    }
}

/// Trait for a modem combining AT UART communication and power control.
pub trait Modem: AtUartTrait {
    /// Powers on the modem hardware.
    fn turn_on(&mut self) -> impl Future<Output = Result<(), Error>>;
}

/// A modem struct combining an AT UART handle and a power pin.
pub struct Bg77<M: AtUartTrait, P: ModemPin> {
    pub bg77: M,
    pub modem_pin: P,
}

impl<M: AtUartTrait, P: ModemPin> Bg77<M, P> {
    pub fn new(bg77: M, modem_pin: P) -> Self {
        Self { bg77, modem_pin }
    }
}

impl<M: AtUartTrait, P: ModemPin> Modem for Bg77<M, P> {
    async fn turn_on(&mut self) -> Result<(), Error> {
        if self.call_at("E0", None).await.is_err() {
            self.modem_pin.set_low();
            Timer::after_secs(1).await;
            self.modem_pin.set_high();
            Timer::after_secs(2).await;
            self.modem_pin.set_low();
            let res = self.read(Duration::from_secs(1)).await?;
            debug!("Modem response: {}", res);
            self.long_call_at("+CFUN=1,0", Duration::from_secs(15)).await?;
            let res = self.read(Duration::from_secs(5)).await?;
            debug!("Modem response: {}", res);
            self.call_at("E0", None).await?;
        }
        Ok(())
    }
}

impl<M: AtUartTrait, P: ModemPin> AtUartTrait for Bg77<M, P> {
    fn spawn_rx(&mut self, urc_handlers: &[UrcHandlerType], spawner: Spawner) {
        self.bg77.spawn_rx(urc_handlers, spawner);
    }

    fn call_at_timeout(
        &mut self,
        command: &str,
        call_timeout: Duration,
        response_timeout: Option<Duration>,
    ) -> impl Future<Output = Result<AtResponse, AtError>> {
        self.bg77.call_at_timeout(command, call_timeout, response_timeout)
    }

    fn call_second_read(
        &mut self,
        msg: &[u8],
        command_prefix: &str,
        second_read: bool,
        timeout: Duration,
    ) -> impl Future<Output = Result<AtResponse, AtError>> {
        self.bg77.call_second_read(msg, command_prefix, second_read, timeout)
    }

    fn read_lines(
        &self,
        timeout: Duration,
    ) -> impl Future<Output = Result<Vec<FromModem, AT_LINES>, AtError>> {
        self.bg77.read_lines(timeout)
    }
}
