#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_time::Duration;
use heapless::String;
use yaroc_nrf52840::device::Device;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let mut device = Device::new();
    info!("Device initialized!");

    let command = String::try_from("ATI\r\n").unwrap();
    let timeout = Duration::from_millis(10);
    device.call_uart1(command, timeout).await;
}
