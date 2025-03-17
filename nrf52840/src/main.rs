#![no_std]
#![no_main]

use core::str::FromStr;

use defmt::*;
use embassy_executor::Spawner;
use embassy_sync::{channel::Channel, mutex::Mutex};
use heapless::String;
use yaroc_nrf52840::{
    self as _,
    bg77_hw::ModemConfig,
    device::Device,
    mqtt::MqttConfig,
    send_punch::{send_punch_event_handler, send_punch_main_loop, SendPunch, SendPunchMutexType},
    si_uart::{si_uart_reader, SiUartChannelType},
};

static SEND_PUNCH_MUTEX: SendPunchMutexType = Mutex::new(None);
static SI_UART_CHANNEL: SiUartChannelType = Channel::new();

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let mqtt_config = MqttConfig {
        name: String::from_str("spe-dev").unwrap(),
        mac_address: String::from_str("cee423506cac").unwrap(),
        ..Default::default()
    };
    let modem_config = ModemConfig::default();
    let device = Device::new();
    info!("Device initialized!");

    let Device {
        bg77,
        temp,
        rng,
        si_uart,
        software_serial,
        ..
    } = device;
    let send_punch = SendPunch::new(
        bg77,
        temp,
        rng,
        &SEND_PUNCH_MUTEX,
        spawner,
        modem_config,
        mqtt_config,
    );
    {
        *(SEND_PUNCH_MUTEX.lock().await) = Some(send_punch);
    }

    spawner.must_spawn(send_punch_main_loop(&SEND_PUNCH_MUTEX));
    spawner.must_spawn(send_punch_event_handler(
        &SEND_PUNCH_MUTEX,
        SI_UART_CHANNEL.receiver(),
    ));
    spawner.must_spawn(si_uart_reader(
        si_uart,
        software_serial,
        SI_UART_CHANNEL.sender(),
    ));
}
