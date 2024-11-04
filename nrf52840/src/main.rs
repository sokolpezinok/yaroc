#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use yaroc_nrf52840::device::Device;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let mut device = Device::new();
    info!("Device initialized!");

    //device.call_uart1("ATI", 10).await.unwrap();
    device.call_uart1("AT+CGATT=1", 10).await.unwrap();
    device.call_uart1("AT+CEREG=2", 10).await.unwrap();
    device
        .call_uart1("AT+CGDCONT=1,\"IP\",trial-nbiot.corp", 1000)
        .await
        .unwrap();
    //device.call_uart1("AT+CGDCONT?", 1000).await.unwrap();
    device.call_uart1("AT+CEREG?", 10).await.unwrap();
    device.call_uart1("AT+QCSQ", 10).await.unwrap();
    device
        .call_uart1("AT+QMTOPEN=3,\"broker.emqx.io\",1883", 10)
        .await
        .unwrap();
    let _ = device.read_uart1(10_000).await;

    device.call_uart1("AT+QMTOPEN?", 100).await.unwrap();
    // Good response: +QMTOPEN: 2,"broker.emqx.io",1883

    info!("\nDone part 1\n");
    device
        .call_uart1("AT+QMTCFG=\"timeout\",0,50,3,0", 10)
        .await
        .unwrap();
    device
        .call_uart1("AT+QMTCONN=3,\"client-embassy\"", 10)
        .await
        .unwrap();
    let _ = device.read_uart1(10_000).await;

    device.call_uart1("AT+QMTCONN?", 10).await.unwrap();
    // Good response +QMTCONN: 2,3

    device
        .call_uart1("AT+QMTPUBEX=3,0,0,0,\"topic/pub\",Hello from embassy", 1000)
        .await
        .unwrap();
    device.call_uart1("AT+QMTDISC=3", 10).await.unwrap();
    let _ = device.read_uart1(10_000).await;
}
