#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use yaroc_nrf52840::device::Device;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let mut device = Device::new();
    info!("Device initialized!");

    device.call_uart1("ATI", 10).await.unwrap();
    device.call_uart1("AT+CGATT=1", 1000).await.unwrap();
    device.call_uart1("AT+CEREG=2", 10).await.unwrap();
    device.call_uart1("AT+CGDCONT=1,\"IP\",trial-nbiot.corp", 1000).await.unwrap();
    //device.call_uart1("AT+CGDCONT?", 1000).await.unwrap();
    device.call_uart1("AT+CEREG?", 10).await.unwrap();
    device.call_uart1("AT+QCSQ", 10).await.unwrap();
    device.call_uart1("AT+QMTDISC=0", 100).await.unwrap();
    device.call_uart1("AT+QMTOPEN=0,\"broker.emqx.io\",1883", 10000).await.unwrap();
    device.call_uart1("AT+QMTOPEN?", 100).await.unwrap();
    device.call_uart1("AT+QMTCFG=\"timeout\",0,50,3,0", 1000).await.unwrap();
    device.call_uart1("AT+QMTCONN=0,\"client-embassy\"", 1000).await.unwrap();
}
