#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_sync::{channel::Channel, mutex::Mutex};
use yaroc_nrf52840::{
    self as _, // global logger + panicking-behavior + memory layout
    bg77::{bg77_main_loop, bg77_urc_handler, BG77Type, Config as BG77Config},
    device::Device,
    si_uart::{si_uart_reader, SiUartChannelType, SiUartType},
};

static BG77_MUTEX: BG77Type = Mutex::new(None);
static SI_UART_MUTEX: SiUartType = Mutex::new(None);
static SI_UART_CHANNEL: SiUartChannelType = Channel::new();

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let bg77_config = BG77Config::default();
    let device = Device::new(spawner, bg77_config);
    info!("Device initialized!");

    let Device { bg77, si_uart, .. } = device;
    {
        *(BG77_MUTEX.lock().await) = Some(bg77);
        *(SI_UART_MUTEX.lock().await) = Some(si_uart);
    }

    spawner.must_spawn(bg77_main_loop(&BG77_MUTEX));
    spawner.must_spawn(bg77_urc_handler(&BG77_MUTEX));
    spawner.must_spawn(si_uart_reader(&SI_UART_MUTEX, &SI_UART_CHANNEL));
}
