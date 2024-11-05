use crate::at_utils::Uart;
use core::fmt::Write;
use defmt::info;
use embassy_nrf::{
    gpio::Output,
    peripherals::{P0_17, UARTE1},
};
use embassy_time::{Duration, Timer};
use heapless::String;

pub struct BG77<'a> {
    uart1: Uart<'a, UARTE1>,
    modem_pin: Output<'a, P0_17>,
}

impl<'a> BG77<'a> {
    pub fn new(uart1: Uart<'a, UARTE1>, modem_pin: Output<'a, P0_17>) -> Self {
        Self { uart1, modem_pin }
    }

    pub async fn experiment(&mut self) {
        let activation_timeout = Duration::from_secs(140);
        let pkt_timeout = Duration::from_secs(35);
        let pkt_timeout_retry = pkt_timeout * 2;
        let minimum_timeout = Duration::from_millis(300);
        let client_id = 3;

        self.uart1.call("AT+CMEE=2", minimum_timeout).await.unwrap();
        self.uart1
            .call("AT+CGATT=1", activation_timeout)
            .await
            .unwrap();
        self.uart1
            .call("AT+CEREG=2", minimum_timeout)
            .await
            .unwrap();
        self.uart1.call("AT+QCSQ", minimum_timeout).await.unwrap();
        //self.uart1
        //    .call("AT+CGDCONT=1,\"IP\",trial-nbiot.corp", minimum_timeout)
        //    .await
        //    .unwrap();
        self.uart1
            .call("AT+CGPADDR=1", minimum_timeout)
            .await
            .unwrap();
        self.uart1.call("AT+CEREG?", minimum_timeout).await.unwrap();
        let mut command = String::<100>::new();
        write!(command, "AT+QMTOPEN={},\"broker.emqx.io\",1883", client_id).unwrap();
        self.uart1.call(&command, activation_timeout).await.unwrap();
        let _ = self.uart1.read(pkt_timeout).await;

        self.uart1
            .call("AT+QMTOPEN?", minimum_timeout)
            .await
            .unwrap();
        // Good response: +QMTOPEN: <client_id>,"broker.emqx.io",1883

        info!("\nDone part 1\n");
        self.uart1
            .call("AT+QMTCFG=\"timeout\",0,45,2,0", minimum_timeout)
            .await
            .unwrap();
        command.clear();
        write!(command, "AT+QMTCONN={},\"client-embassy\"", client_id).unwrap();
        self.uart1.call(&command, pkt_timeout).await.unwrap();
        let _ = self.uart1.read(pkt_timeout).await;
        // +QMTCONN: <client_id>,0,0

        self.uart1
            .call("AT+QMTCONN?", minimum_timeout)
            .await
            .unwrap();
        // Good response +QMTCONN: <client_id>,3

        command.clear();
        write!(
            command,
            "AT+QMTPUBEX={},0,0,0,\"topic/pub\",Hello from embassy",
            client_id
        )
        .unwrap();
        self.uart1.call(&command, pkt_timeout_retry).await.unwrap();
        command.clear();
        write!(command, "AT+QMTDISC={}", client_id).unwrap();
        self.uart1.call(&command, minimum_timeout).await.unwrap();

        loop {
            let _ = self.uart1.read(pkt_timeout).await;
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
