use crate::{at_utils::Uart, error::Error};
use core::fmt::Write;
use defmt::info;
use embassy_nrf::{
    gpio::Output,
    peripherals::{P0_17, UARTE1},
};
use embassy_time::{Duration, Timer};
use heapless::String;

static MINIMUM_TIMEOUT: Duration = Duration::from_millis(300);
const CLIENT_ID: u32 = 0;

pub struct BG77<'a> {
    uart1: Uart<'a, UARTE1>,
    modem_pin: Output<'a, P0_17>,
    pkt_timeout: Duration,
    activation_timeout: Duration,
}

impl<'a> BG77<'a> {
    pub fn new(uart1: Uart<'a, UARTE1>, modem_pin: Output<'a, P0_17>) -> Self {
        let activation_timeout = Duration::from_secs(140);
        let pkt_timeout = Duration::from_secs(35);
        Self {
            uart1,
            modem_pin,
            activation_timeout,
            pkt_timeout,
        }
    }

    pub async fn config(&mut self) -> Result<(), Error> {
        self.uart1
            .call("AT+CGATT=1", self.activation_timeout)
            .await?;
        self.uart1.call("AT+CEREG=2", MINIMUM_TIMEOUT).await?;
        self.uart1
            .call("AT+CGDCONT=1,\"IP\",trial-nbiot.corp", MINIMUM_TIMEOUT)
            .await?;
        self.uart1.call("AT+CGPADDR=1", MINIMUM_TIMEOUT).await?;
        Ok(())
    }

    pub async fn mqtt_connect(&mut self) -> Result<(), Error> {
        let mut command = String::<100>::new();
        write!(command, "AT+QMTOPEN={},\"broker.emqx.io\",1883", CLIENT_ID).unwrap();
        self.uart1.call(&command, self.activation_timeout).await?;
        let _ = self.uart1.read(self.pkt_timeout).await;

        self.uart1.call("AT+QMTOPEN?", MINIMUM_TIMEOUT).await?;
        // Good response: +QMTOPEN: <client_id>,"broker.emqx.io",1883

        info!("\nDone part 1\n");
        self.uart1
            .call("AT+QMTCFG=\"timeout\",0,45,2,0", MINIMUM_TIMEOUT)
            .await?;
        command.clear();
        write!(command, "AT+QMTCONN={},\"client-embassy\"", CLIENT_ID).unwrap();
        self.uart1.call(&command, self.pkt_timeout).await.unwrap();
        let _ = self.uart1.read(self.pkt_timeout).await;
        // +QMTCONN: <client_id>,0,0

        self.uart1.call("AT+QMTCONN?", MINIMUM_TIMEOUT).await?;
        // Good response +QMTCONN: <client_id>,3
        Ok(())
    }

    pub async fn mqtt_disconnect(&mut self) {
        let mut command = String::<100>::new();
        write!(command, "AT+QMTDISC={}", CLIENT_ID).unwrap();
        self.uart1.call(&command, MINIMUM_TIMEOUT).await.unwrap();
    }

    pub async fn experiment(&mut self) {
        let _ = self.config().await;
        let _ = self.mqtt_connect().await;

        // Info
        self.uart1.call("AT+QCSQ", MINIMUM_TIMEOUT).await.unwrap();
        self.uart1.call("AT+CEREG?", MINIMUM_TIMEOUT).await.unwrap();
        let mut command = String::<100>::new();
        write!(
            command,
            "AT+QMTPUBEX={},0,0,0,\"topic/pub\",Hello from embassy",
            CLIENT_ID
        )
        .unwrap();
        self.uart1
            .call(&command, self.pkt_timeout * 2)
            .await
            .unwrap();
        self.mqtt_disconnect().await;

        loop {
            let _ = self.uart1.read(self.pkt_timeout).await;
        }
    }

    async fn turn_on(&mut self) {
        self.modem_pin.set_low();
        Timer::after_millis(1000).await;
        self.modem_pin.set_high();
        Timer::after_millis(2000).await;
        self.modem_pin.set_low();
        Timer::after_millis(100).await;
    }
}
