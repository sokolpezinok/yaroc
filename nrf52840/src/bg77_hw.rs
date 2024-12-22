use defmt::info;
use embassy_nrf::{gpio::Output, peripherals::P0_17};
use embassy_time::{Duration, Timer};
use heapless::Vec;
use yaroc_common::at::uart::AtUart;
use yaroc_common::at::{
    response::{AtResponse, FromModem},
    uart::Tx,
};

static MINIMUM_TIMEOUT: Duration = Duration::from_millis(300);

pub struct Bg77Hw<T: Tx> {
    uart1: AtUart<T>,
    modem_pin: Output<'static, P0_17>,
}

impl<T: Tx> Bg77Hw<T> {
    pub fn new(uart1: AtUart<T>, modem_pin: Output<'static, P0_17>) -> Self {
        Self { uart1, modem_pin }
    }

    pub async fn simple_call_at(
        &mut self,
        cmd: &str,
        response_timeout: Option<Duration>,
    ) -> crate::Result<AtResponse> {
        match response_timeout {
            None => Ok(self.uart1.call_at(cmd, MINIMUM_TIMEOUT).await?),
            Some(response_timeout) => Ok(self
                .uart1
                .call_at_with_response(cmd, MINIMUM_TIMEOUT, response_timeout)
                .await?),
        }
    }

    pub async fn call_at(
        &mut self,
        cmd: &str,
        timeout: Duration,
    ) -> yaroc_common::Result<AtResponse> {
        self.uart1.call_at(cmd, timeout).await
    }

    pub async fn call(&mut self, msg: &[u8]) -> yaroc_common::Result<Vec<FromModem, 4>> {
        self.uart1.call(msg, MINIMUM_TIMEOUT).await
    }

    pub async fn turn_on(&mut self) -> crate::Result<()> {
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
