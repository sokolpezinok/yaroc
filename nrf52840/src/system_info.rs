use defmt::error;
use embassy_futures::select::{Either3, select3};
use embassy_time::{Duration, Ticker};
use yaroc_common::status::{TEMPERATURE, Temp};

use crate::{
    ble::Ble,
    send_punch::{Command, EVENT_CHANNEL},
};

/// A struct for reading the temperature from the softdevice.
pub struct SoftdeviceTemp {
    ble: Ble,
}

impl SoftdeviceTemp {
    /// Creates a new `SoftdeviceTemp`.
    pub fn new(ble: Ble) -> Self {
        Self { ble }
    }
}

impl Temp for SoftdeviceTemp {
    async fn cpu_temperature(&mut self) -> crate::Result<f32> {
        self.ble.temperature()
    }
}

/// The type of the temperature sensor.
pub type OwnTemp = SoftdeviceTemp;

/// A task that periodically updates the system info.
#[embassy_executor::task]
pub async fn sysinfo_update(mut temp: OwnTemp) {
    // Initial commands in the beginning
    EVENT_CHANNEL.send(Command::SynchronizeTime).await;
    EVENT_CHANNEL.send(Command::BatteryUpdate).await;
    let temp_sender = TEMPERATURE.sender();
    let mut temperature_ticker = Ticker::every(Duration::from_secs(120));
    let mut time_sync_ticker = Ticker::every(Duration::from_secs(1800));
    let mut battery_update = Ticker::every(Duration::from_secs(120));
    loop {
        match select3(
            temperature_ticker.next(),
            time_sync_ticker.next(),
            battery_update.next(),
        )
        .await
        {
            Either3::First(_) => {
                let _ = temp
                    .cpu_temperature()
                    .await
                    .map(|t| temp_sender.send(t))
                    .inspect_err(|err| error!("Temperature update failed: {}", err));
            }
            Either3::Second(_) => EVENT_CHANNEL.send(Command::SynchronizeTime).await,
            Either3::Third(_) => EVENT_CHANNEL.send(Command::BatteryUpdate).await,
        }
    }
}
