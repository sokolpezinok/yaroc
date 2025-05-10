#![no_std]
#![no_main]

use core::str::FromStr;

use defmt::info;
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
    system_info::temperature_update,
};

static SEND_PUNCH_MUTEX: SendPunchMutexType = Mutex::new(None);
static SI_UART_CHANNEL: SiUartChannelType = Channel::new();

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let device = Device::new();
    let Device {
        bg77,
        si_uart,
        #[cfg(not(feature = "bluetooth-le"))]
        temp,
        #[cfg(feature = "bluetooth-le")]
        ble,
        ..
    } = device;

    #[cfg(feature = "bluetooth-le")]
    let mac_address = ble.get_mac_address();
    #[cfg(not(feature = "bluetooth-le"))]
    let mac_address: String<12> = String::from_str("cee423506cac").unwrap();

    info!("Device initialized, MAC address: {}", mac_address.as_str());
    let mqtt_config = MqttConfig {
        name: String::from_str("spe06").unwrap(),
        mac_address,
        ..Default::default()
    };
    let modem_config = ModemConfig::default();

    #[cfg(not(feature = "bluetooth-le"))]
    let temp = yaroc_nrf52840::system_info::NrfTemp::new(temp);
    #[cfg(feature = "bluetooth-le")]
    let temp = yaroc_nrf52840::system_info::SoftdeviceTemp::new(ble);

    let send_punch = SendPunch::new(
        bg77,
        &SEND_PUNCH_MUTEX,
        spawner,
        modem_config,
        mqtt_config.clone(),
    );
    {
        *(SEND_PUNCH_MUTEX.lock().await) = Some(send_punch);
    }

    spawner.must_spawn(send_punch_main_loop(&SEND_PUNCH_MUTEX, mqtt_config));
    spawner.must_spawn(send_punch_event_handler(
        &SEND_PUNCH_MUTEX,
        SI_UART_CHANNEL.receiver(),
    ));
    spawner.must_spawn(temperature_update(temp));
    spawner.must_spawn(si_uart_reader(si_uart, SI_UART_CHANNEL.sender()));
}
