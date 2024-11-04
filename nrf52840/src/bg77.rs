use crate::at_utils::Uart;
use defmt::*;
use embassy_nrf::{
    gpio::Output,
    peripherals::{P0_17, UARTE1},
};
use embassy_time::Timer;

pub struct BG77<'a> {
    uart1: Uart<'a, UARTE1>,
    modem_pin: Output<'a, P0_17>,
}

impl<'a> BG77<'a> {
    pub fn new(uart1: Uart<'a, UARTE1>, modem_pin: Output<'a, P0_17>) -> Self {
        Self { uart1, modem_pin }
    }

    pub async fn experiment(&mut self) {
        //device.call_uart1("ATI", 10).await.unwrap();
        self.uart1.call("AT+CGATT=1", 10).await.unwrap();
        self.uart1.call("AT+CEREG=2", 10).await.unwrap();
        self.uart1
            .call("AT+CGDCONT=1,\"IP\",trial-nbiot.corp", 1000)
            .await
            .unwrap();
        //self.uart1.call_self.uart1("AT+CGDCONT?", 1000).await.unwrap();
        self.uart1.call("AT+CEREG?", 10).await.unwrap();
        self.uart1.call("AT+QCSQ", 10).await.unwrap();
        self.uart1
            .call("AT+QMTOPEN=3,\"broker.emqx.io\",1883", 10)
            .await
            .unwrap();
        let _ = self.uart1.read(10_000).await;

        self.uart1.call("AT+QMTOPEN?", 100).await.unwrap();
        // Good response: +QMTOPEN: 2,"broker.emqx.io",1883

        info!("\nDone part 1\n");
        self.uart1
            .call("AT+QMTCFG=\"timeout\",0,50,3,0", 10)
            .await
            .unwrap();
        self.uart1
            .call("AT+QMTCONN=3,\"client-embassy\"", 10)
            .await
            .unwrap();
        let _ = self.uart1.read(10_000).await;

        self.uart1.call("AT+QMTCONN?", 10).await.unwrap();
        // Good response +QMTCONN: 2,3

        self.uart1
            .call("AT+QMTPUBEX=3,0,0,0,\"topic/pub\",Hello from embassy", 1000)
            .await
            .unwrap();
        self.uart1.call("AT+QMTDISC=3", 10).await.unwrap();
        let _ = self.uart1.read(10_000).await;
    }

    async fn turn_on_bg77(&mut self) {
        self.modem_pin.set_low();
        Timer::after_millis(1000).await;
        self.modem_pin.set_high();
        Timer::after_millis(2000).await;
        self.modem_pin.set_low();
        Timer::after_millis(100).await;
    }
}
