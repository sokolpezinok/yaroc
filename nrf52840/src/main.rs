#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_sync::mutex::Mutex;
use yaroc_nrf52840::{
    bg77::{bg77_main_loop, BG77Type},
    device::Device,
};

static BG77_MUTEX: BG77Type = Mutex::new(None);

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let device = Device::new(spawner);
    info!("Device initialized!");

    let Device { bg77, .. } = device;
    {
        *(BG77_MUTEX.lock().await) = Some(bg77);
    }

    spawner.must_spawn(bg77_main_loop(&BG77_MUTEX));
}
