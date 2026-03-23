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
    bg77::{
        modem_manager::ModemConfig,
        mqtt::{MqttConfig, MqttConfigReduced},
    },
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

    ble.spawn(spawner);

    let mut mqtt_config = MqttConfig {
        name: format!(24; "nrf52840-{mac_address}").unwrap(),
        mac_address,
        ..Default::default()
    };
    info!("Device initialized: {}", mqtt_config.name.as_str(),);

    static FLASH_MUTEX: StaticCell<embassy_sync::mutex::Mutex<RawMutex, nrf_softdevice::Flash>> =
        StaticCell::new();
    let flash_mutex = FLASH_MUTEX.init(flash_mutex);
    let mut flash = NrfFlash::new(flash_mutex);
    let mut buffer = [0; 4096];
    {
        if let Ok(Some(reduced_config)) =
            flash.read::<MqttConfigReduced>(ValueIndex::MqttConfig, &mut buffer).await
        {
            mqtt_config.update(reduced_config);
        }
    }

    let modem_config = match flash.read(ValueIndex::ModemConfig, &mut buffer).await {
        Ok(config) => config.unwrap_or_default(),
        Err(err) => {
            error!("Error while reading modem config from flash: {}", err);
            let _ = flash
                .write(ValueIndex::ModemConfig, ModemConfig::default())
                .await
                .inspect_err(|e| error!("Error while writing modem config: {}", e));
            ModemConfig::default()
        }
    };

    usb.spawn(spawner);
    spawner
        .spawn(minicallhome_loop(mqtt_config.minicallhome_interval).expect("Failed to spawn task"));
    spawner.spawn(read_si_uart(si_uart, SI_UART_CHANNEL.sender()).expect("Failed to spawn task"));

    let send_punch_fn = Bg77SendPunchFn::new(mqtt_config.packet_timeout);
    let backoff_retries = BackoffRetries::new(
        send_punch_fn,
        Duration::from_secs(10),
        PUNCH_QUEUE_SIZE - 1,
        spawner,
    );
    spawner.spawn(backoff_retries_loop(backoff_retries).expect("Failed to spawn task"));

    let send_punch = SendPunch::new(bg77, modem_pin, spawner, mqtt_config, modem_config, flash);
    {
        *(SEND_PUNCH_MUTEX.lock().await) = Some(send_punch);
    }
    spawner
        .spawn(send_punch_event_handler(SI_UART_CHANNEL.receiver()).expect("Failed to spawn task"));

    let temp = SoftdeviceTemp::new(ble);
    spawner.spawn(sysinfo_update(temp).expect("Failed to spawn task"));
    info!("All background tasks are running");
}
