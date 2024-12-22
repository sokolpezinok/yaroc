use defmt::info;
use embassy_time::{Duration, Timer};
use heapless::Vec;
use yaroc_common::at::{
    response::{AtResponse, FromModem},
    uart::Tx,
};

use crate::bg77::BG77;
use crate::status::Temp;

static BG77_MINIMUM_TIMEOUT: Duration = Duration::from_millis(300);

pub trait ModemHw {
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
    ) -> impl core::future::Future<Output = yaroc_common::Result<AtResponse>>;

    /// Sends a raw message to the modem.
    ///
    /// Waits only a short time for a response (non-configurable).
    fn call(
        &mut self,
        msg: &[u8],
    ) -> impl core::future::Future<Output = yaroc_common::Result<Vec<FromModem, 4>>>;

    fn turn_on(&mut self) -> impl core::future::Future<Output = crate::Result<()>>;
}

impl<S: Temp, T: Tx> ModemHw for BG77<S, T> {
    async fn simple_call_at(
        &mut self,
        cmd: &str,
        response_timeout: Option<Duration>,
    ) -> crate::Result<AtResponse> {
        match response_timeout {
            None => Ok(self.uart1.call_at(cmd, BG77_MINIMUM_TIMEOUT).await?),
            Some(response_timeout) => Ok(self
                .uart1
                .call_at_with_response(cmd, BG77_MINIMUM_TIMEOUT, response_timeout)
                .await?),
        }
    }

    async fn call_at(&mut self, cmd: &str, timeout: Duration) -> yaroc_common::Result<AtResponse> {
        self.uart1.call_at(cmd, timeout).await
    }

    async fn call(&mut self, msg: &[u8]) -> yaroc_common::Result<Vec<FromModem, 4>> {
        self.uart1.call(msg, BG77_MINIMUM_TIMEOUT).await
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