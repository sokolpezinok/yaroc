#![no_std]
#![no_main]

use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_sync::channel::Channel;
use embassy_time::Duration;
use heapless::format;
use static_cell::StaticCell;
use yaroc_common::{
    RawMutex,
    backoff::{BackoffRetries, BatchedPunches, PUNCH_QUEUE_SIZE},
    bg77::{modem_manager::ModemConfig, mqtt::MqttConfig},
    error::Error,
    send_punch::SendPunch,
};
use yaroc_nrf52840::{
    self as _,
    device::Device,
    flash::{Flash, NrfFlash, ValueIndex},
    send_punch::{
        Bg77SendPunchFn, SEND_PUNCH_MUTEX, backoff_retries_loop, send_punch_event_handler,
    },
    si_uart::read_si_uart,
    system_info::{SoftdeviceTemp, minicallhome_loop, sysinfo_update},
};

/// A channel for the SI UART.
static SI_UART_CHANNEL: Channel<RawMutex, Result<BatchedPunches, Error>, 24> = Channel::new();

/// The main entry point of the application.
#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let device = Device::default();
    let Device {
        mac_address,
        bg77,
        modem_pin,
        si_uart,
        ble,
        flash_mutex,
        usb,
        ..
    } = device;

    ble.must_spawn(spawner);

    static FLASH_MUTEX: StaticCell<embassy_sync::mutex::Mutex<RawMutex, nrf_softdevice::Flash>> =
        StaticCell::new();
    let flash_mutex = FLASH_MUTEX.init(flash_mutex);

    let mut flash = NrfFlash::new(flash_mutex);
    let mut buffer = [0; 4096];

    let mqtt_config = MqttConfig {
        name: format!(24; "nrf52840-{mac_address}").unwrap(),
        mac_address,
        ..Default::default()
    };
    info!("Device initialized: {}", mqtt_config.name.as_str(),);

    let modem_config = match flash.read(ValueIndex::ModemConfig, &mut buffer).await {
        Ok(config) => config.unwrap_or_default(),
        Err(err) => {
            error!("Error while reading modem config from flash: {}", err);
            let mut buffer = [0; 4096];
            let _ = flash
                .write(ValueIndex::ModemConfig, ModemConfig::default(), &mut buffer)
                .await
                .inspect_err(|e| error!("Error while writing modem config: {}", e));
            ModemConfig::default()
        }
    };

    usb.must_spawn(spawner);
    spawner.must_spawn(minicallhome_loop(mqtt_config.minicallhome_interval));
    spawner.must_spawn(read_si_uart(si_uart, SI_UART_CHANNEL.sender()));

    let send_punch_fn = Bg77SendPunchFn::new(mqtt_config.packet_timeout);
    let backoff_retries = BackoffRetries::new(
        send_punch_fn,
        Duration::from_secs(10),
        PUNCH_QUEUE_SIZE - 1,
        spawner,
    );
    spawner.must_spawn(backoff_retries_loop(backoff_retries));

    let send_punch = SendPunch::new(bg77, modem_pin, spawner, mqtt_config, modem_config, flash);
    {
        *(SEND_PUNCH_MUTEX.lock().await) = Some(send_punch);
    }
    spawner.must_spawn(send_punch_event_handler(SI_UART_CHANNEL.receiver()));

    let temp = SoftdeviceTemp::new(ble);
    spawner.must_spawn(sysinfo_update(temp));
    info!("All background tasks are running");
}
