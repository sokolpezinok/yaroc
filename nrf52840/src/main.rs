#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use yaroc_nrf52840::device::Device;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let mut device = Device::new();
    info!("Device initialized!");

    device.read_uart1().await;
}
