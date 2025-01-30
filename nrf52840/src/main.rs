#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_sync::{channel::Channel, mutex::Mutex};
use yaroc_nrf52840::{
    self as _,
    device::Device,
    mqtt::MqttConfig,
    send_punch::{send_punch_event_handler, send_punch_main_loop, SendPunch, SendPunchMutexType},
    si_uart::{si_uart_reader, SiUartChannelType},
};

static SEND_PUNCH_MUTEX: SendPunchMutexType = Mutex::new(None);
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
    let send_punch = SendPunch::new(bg77, temp, &SEND_PUNCH_MUTEX, &spawner, mqtt_config);
    {
        *(SEND_PUNCH_MUTEX.lock().await) = Some(send_punch);
    }

    spawner.must_spawn(send_punch_main_loop(&SEND_PUNCH_MUTEX));
    spawner.must_spawn(send_punch_event_handler(
        &SEND_PUNCH_MUTEX,
        &SI_UART_CHANNEL,
    ));
    spawner.must_spawn(si_uart_reader(si_uart, software_serial, &SI_UART_CHANNEL));
}
