use crate::{at_utils::AtUart, error::Error};
use defmt::unwrap;
use embassy_executor::Spawner;
use embassy_nrf::{
    gpio::Output,
    peripherals::{P0_17, TIMER0, UARTE1},
    uarte::{UarteRxWithIdle, UarteTx},
};
use embassy_time::{Duration, Timer};
use heapless::format;

static MINIMUM_TIMEOUT: Duration = Duration::from_millis(300);
const CLIENT_ID: u32 = 0;

pub struct BG77 {
    uart1: AtUart,
    _modem_pin: Output<'static, P0_17>,
    pkt_timeout: Duration,
    activation_timeout: Duration,
}

fn callback_dispatcher(prefix: &str, _rest: &str) -> bool {
    match prefix {
        "QMTSTAT" => true,
        _ => false,
    }
}

impl BG77 {
    pub fn new(
        rx1: UarteRxWithIdle<'static, UARTE1, TIMER0>,
        tx1: UarteTx<'static, UARTE1>,
        modem_pin: Output<'static, P0_17>,
        spawner: &Spawner,
    ) -> Self {
        let uart1 = AtUart::new(rx1, tx1, callback_dispatcher, spawner);
        let activation_timeout = Duration::from_secs(14); // 140
        let pkt_timeout = Duration::from_secs(8); // 30
        Self {
            uart1,
            _modem_pin: modem_pin,
            activation_timeout,
            pkt_timeout,
        }
    }

    pub async fn config(&mut self) -> Result<(), Error> {
        self.uart1.call("ATE0", MINIMUM_TIMEOUT).await?;
        self.uart1
            .call("AT+CGATT=1", self.activation_timeout)
            .await?;
        self.uart1.call("AT+CEREG=2", MINIMUM_TIMEOUT).await?;
        let _ = self
            .uart1
            .call("AT+CGDCONT=1,\"IP\",trial-nbiot.corp", MINIMUM_TIMEOUT)
            .await;
        self.uart1.call("AT+CGPADDR=1", MINIMUM_TIMEOUT).await?;
        Ok(())
    }

    pub async fn mqtt_connect(&mut self) -> Result<(), Error> {
        let at_command = format!(100; "AT+QMTOPEN={CLIENT_ID},\"broker.emqx.io\",1883").unwrap();
        self.uart1
            .call_with_response(&at_command, MINIMUM_TIMEOUT, self.activation_timeout)
            .await?;

        self.uart1.call("AT+QMTOPEN?", MINIMUM_TIMEOUT).await?;
        // Good response: +QMTOPEN: <client_id>,"broker.emqx.io",1883

        let command = format!(50;
            "AT+QMTCFG=\"timeout\",{CLIENT_ID},{},2,1",
            self.pkt_timeout.as_secs()
        )
        .unwrap();
        self.uart1.call(&command, MINIMUM_TIMEOUT).await?;
        let command = format!(50;
            "AT+QMTCFG=\"keepalive\",{CLIENT_ID},{}",
            (self.pkt_timeout * 3).as_secs()
        )
        .unwrap();
        self.uart1.call(&command, MINIMUM_TIMEOUT).await?;
        let command = format!(50; "AT+QMTCONN={CLIENT_ID},\"client-embassy\"").unwrap();
        let _ = self
            .uart1
            .call_with_response(&command, MINIMUM_TIMEOUT, self.pkt_timeout)
            .await;
        // +QMTCONN: <client_id>,0,0

        self.uart1.call("AT+QMTCONN?", MINIMUM_TIMEOUT).await?;
        // Good response +QMTCONN: <client_id>,3
        Ok(())
    }

    pub async fn mqtt_disconnect(&mut self) -> Result<(), Error> {
        let command = format!(50; "AT+QMTDISC={CLIENT_ID}").unwrap();
        self.uart1.call(&command, MINIMUM_TIMEOUT).await?;
        Ok(())
    }

    pub async fn signal_info(&mut self) -> Result<(), Error> {
        self.uart1.call("AT+QCSQ", MINIMUM_TIMEOUT).await?;
        self.uart1.call("AT+CEREG?", MINIMUM_TIMEOUT).await?;
        self.uart1.call("AT+QMTCONN?", MINIMUM_TIMEOUT).await?;
        let _ = self.uart1.read(self.pkt_timeout).await;
        Ok(())
    }

    pub async fn experiment(&mut self) {
        unwrap!(self.config().await);
        let _ = self.mqtt_connect().await;

        let command = format!(100;
            "AT+QMTPUBEX={CLIENT_ID},0,0,0,\"topic/pub\",\"Hello from embassy\""
        )
        .unwrap();
        let _ = self.uart1.call(&command, self.pkt_timeout * 2).await;
        for _ in 0..5 {
            unwrap!(self.signal_info().await);
        }
        unwrap!(self.mqtt_disconnect().await);
    }

    async fn _turn_on(&mut self) {
        self._modem_pin.set_low();
        Timer::after_millis(1000).await;
        self._modem_pin.set_high();
        Timer::after_millis(2000).await;
        self._modem_pin.set_low();
        Timer::after_millis(100).await;
    }
}
