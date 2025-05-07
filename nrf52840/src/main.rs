#![no_std]
#![no_main]

use core::str::FromStr;

use defmt::info;
use embassy_executor::Spawner;
use embassy_sync::{channel::Channel, mutex::Mutex};
use heapless::String;
#[cfg(feature = "bluetooth-le")]
use yaroc_nrf52840::ble::Ble;
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
    let device = Device::new();

    #[cfg(feature = "bluetooth-le")]
    let mac_address = {
        let ble = Ble::new();
        ble.get_mac_address()
    };
    #[cfg(not(feature = "bluetooth-le"))]
    let mac_address: String<12> = String::from_str("cee423506cac").unwrap();

    info!("Device initialized, MAC address: {}", mac_address.as_str());
    let mqtt_config = MqttConfig {
        name: String::from_str("spe-dev").unwrap(),
        mac_address,
        ..Default::default()
    };
    let modem_config = ModemConfig::default();
    let Device {
        bg77,
        temp,
        si_uart,
        ..
    } = device;
    let send_punch = SendPunch::new(
        bg77,
        temp,
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
    spawner.must_spawn(si_uart_reader(si_uart, SI_UART_CHANNEL.sender()));
}
