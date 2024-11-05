#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use yaroc_nrf52840::device::Device;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let device = Device::new();
    info!("Device initialized!");

    let Device { mut bg77, .. } = device;

    bg77.experiment().await;
}
