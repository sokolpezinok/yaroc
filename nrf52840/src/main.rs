#![no_std]
#![no_main]

use core::str::FromStr;

use defmt::info;
use embassy_executor::Spawner;
use embassy_sync::{channel::Channel, mutex::Mutex};
use heapless::String;
use yaroc_common::bg77::hw::ModemConfig;
use yaroc_nrf52840::{
    self as _,
    device::Device,
    mqtt::MqttConfig,
    send_punch::{SendPunch, SendPunchMutexType, minicallhome_loop, send_punch_event_handler},
    si_uart::{SiUartChannelType, si_uart_reader},
    system_info::sysinfo_update,
};

/// A mutex for the `SendPunch` struct.
static SEND_PUNCH_MUTEX: SendPunchMutexType = Mutex::new(None);
/// A channel for the SI UART.
static SI_UART_CHANNEL: SiUartChannelType = Channel::new();

/// The main entry point of the application.
#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let modem_config = ModemConfig::default();
    let device = Device::new(modem_config);
    let Device {
        mac_address,
        bg77,
        si_uart,
        ble,
        ..
    } = device;

    info!("Device initialized, MAC address: {}", mac_address.as_str());
    let mqtt_config = MqttConfig {
        name: String::from_str("spe06").unwrap(),
        mac_address,
        ..Default::default()
    };

    spawner.must_spawn(minicallhome_loop(mqtt_config.minicallhome_interval));

    let send_punch = SendPunch::new(bg77, &SEND_PUNCH_MUTEX, spawner, mqtt_config);
    {
        *(SEND_PUNCH_MUTEX.lock().await) = Some(send_punch);
    }
    spawner.must_spawn(send_punch_event_handler(
        &SEND_PUNCH_MUTEX,
        SI_UART_CHANNEL.receiver(),
    ));
    spawner.must_spawn(si_uart_reader(si_uart, SI_UART_CHANNEL.sender()));

    let temp = yaroc_nrf52840::system_info::SoftdeviceTemp::new(ble);
    spawner.must_spawn(sysinfo_update(temp));
}
