#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use yaroc_nrf52840::device::Device;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let mut device = Device::new();
    info!("Device initialized!");

    let uart1 = device.uart1();
    //device.call_uart1("ATI", 10).await.unwrap();
    uart1.call("AT+CGATT=1", 10).await.unwrap();
    uart1.call("AT+CEREG=2", 10).await.unwrap();
    uart1
        .call("AT+CGDCONT=1,\"IP\",trial-nbiot.corp", 1000)
        .await
        .unwrap();
    //uart1.call_uart1("AT+CGDCONT?", 1000).await.unwrap();
    uart1.call("AT+CEREG?", 10).await.unwrap();
    uart1.call("AT+QCSQ", 10).await.unwrap();
    uart1
        .call("AT+QMTOPEN=3,\"broker.emqx.io\",1883", 10)
        .await
        .unwrap();
    let _ = uart1.read(10_000).await;

    uart1.call("AT+QMTOPEN?", 100).await.unwrap();
    // Good response: +QMTOPEN: 2,"broker.emqx.io",1883

    info!("\nDone part 1\n");
    uart1
        .call("AT+QMTCFG=\"timeout\",0,50,3,0", 10)
        .await
        .unwrap();
    uart1
        .call("AT+QMTCONN=3,\"client-embassy\"", 10)
        .await
        .unwrap();
    let _ = uart1.read(10_000).await;

    uart1.call("AT+QMTCONN?", 10).await.unwrap();
    // Good response +QMTCONN: 2,3

    uart1
        .call("AT+QMTPUBEX=3,0,0,0,\"topic/pub\",Hello from embassy", 1000)
        .await
        .unwrap();
    uart1.call("AT+QMTDISC=3", 10).await.unwrap();
    let _ = uart1.read(10_000).await;
}
