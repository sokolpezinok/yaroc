#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_sync::{channel::Channel, mutex::Mutex};
use yaroc_nrf52840::{
    self as _, // global logger + panicking-behavior + memory layout
    bg77::{bg77_event_handler, bg77_main_loop, BG77MutexType, MqttConfig, SendPunch},
    device::Device,
    si_uart::{si_uart_reader, SiUartChannelType},
};

static BG77_MUTEX: BG77MutexType = Mutex::new(None);
static SI_UART_CHANNEL: SiUartChannelType = Channel::new();

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let mqtt_config = MqttConfig::default();
    let device = Device::new();
    info!("Device initialized!");

    let Device {
        bg77,
        temp,
        si_uart,
        software_serial,
        ..
    } = device;
    let send_punch = SendPunch::new(bg77, temp, &spawner, mqtt_config);
    {
        *(BG77_MUTEX.lock().await) = Some(send_punch);
    }

    spawner.must_spawn(bg77_main_loop(&BG77_MUTEX));
    spawner.must_spawn(bg77_event_handler(&BG77_MUTEX, &SI_UART_CHANNEL));
    spawner.must_spawn(si_uart_reader(si_uart, software_serial, &SI_UART_CHANNEL));
}
