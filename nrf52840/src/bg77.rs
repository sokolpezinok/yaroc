use crate::{at_utils::Uart, error::Error};
use defmt::{info, unwrap};
use embassy_nrf::{
    gpio::Output,
    peripherals::{P0_17, UARTE1},
};
use embassy_time::{Duration, Timer};
use heapless::{format, String};

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
        let at_command = format!(100; "AT+QMTOPEN={CLIENT_ID},\"broker.emqx.io\",1883").unwrap();
        self.uart1
            .call(&at_command, self.activation_timeout)
            .await?;

        let _ = self.uart1.read(self.pkt_timeout).await;

        self.uart1.call("AT+QMTOPEN?", MINIMUM_TIMEOUT).await?;
        // Good response: +QMTOPEN: <client_id>,"broker.emqx.io",1883

        info!("\nDone part 1\n");
        let command = format!(50;
            "AT+QMTCFG=\"timeout\",{CLIENT_ID},{},2,1",
            self.pkt_timeout.as_secs()
        )
        .unwrap();
        self.uart1.call(&command, MINIMUM_TIMEOUT).await?;
        let command = format!(50;
            "AT+QMTCFG=\"keepalive\",{}",
            (self.pkt_timeout * 3).as_secs()
        )
        .unwrap();
        self.uart1.call(&command, MINIMUM_TIMEOUT).await?;
        let command = format!(50; "AT+QMTCONN={CLIENT_ID},\"client-embassy\"").unwrap();
        self.uart1.call(&command, self.pkt_timeout).await?;
        let _ = self.uart1.read(self.pkt_timeout).await;
        // +QMTCONN: <client_id>,0,0

        self.uart1.call("AT+QMTCONN?", MINIMUM_TIMEOUT).await?;
        // Good response +QMTCONN: <client_id>,3
        Ok(())
    }

    pub async fn mqtt_disconnect(&mut self) -> Result<(), Error> {
        let command = format!(50; "AT+QMTDISC={CLIENT_ID}").unwrap();
        self.uart1.call(&command, MINIMUM_TIMEOUT).await
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
        unwrap!(self.mqtt_connect().await);

        let command = format!(100;
            "AT+QMTPUBEX={CLIENT_ID},0,0,0,\"topic/pub\",Hello from embassy"
        )
        .unwrap();
        self.uart1
            .call(&command, self.pkt_timeout * 2)
            .await
            .unwrap();

        for _ in 0..10 {
            unwrap!(self.signal_info().await);
        }
        unwrap!(self.mqtt_disconnect().await);
    }

    async fn _turn_on(&mut self) {
        self.modem_pin.set_low();
        Timer::after_millis(1000).await;
        self.modem_pin.set_high();
        Timer::after_millis(2000).await;
        self.modem_pin.set_low();
        Timer::after_millis(100).await;
    }
}
