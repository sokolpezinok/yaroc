use defmt::error;
use embassy_futures::select::{Either3, select3};
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Ticker};
use yaroc_common::{
    RawMutex,
    send_punch::{COMMAND_CHANNEL, SendPunchCommand},
    status::{TEMPERATURE, Temp},
};

use crate::ble::Ble;

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
    COMMAND_CHANNEL.send(SendPunchCommand::SynchronizeTime).await;
    COMMAND_CHANNEL.send(SendPunchCommand::BatteryUpdate).await;
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
            Either3::Second(_) => COMMAND_CHANNEL.send(SendPunchCommand::SynchronizeTime).await,
            Either3::Third(_) => COMMAND_CHANNEL.send(SendPunchCommand::BatteryUpdate).await,
        }
    }
}

/// A signal used to trigger a MiniCallHome event.
pub static MCH_SIGNAL: Signal<RawMutex, Instant> = Signal::new();

/// A task that periodically triggers a `MiniCallHome` event.
///
/// # Arguments
///
/// * `minicallhome_interval`: The interval at which to trigger the `MiniCallHome` event.
#[embassy_executor::task]
pub async fn minicallhome_loop(minicallhome_interval: Duration) {
    let mut mch_ticker = Ticker::every(minicallhome_interval);
    loop {
        // We use Signal, so that MiniCallHome requests do not queue up. If we do not fulfill a few
        // requests, e.g. during a long network search, it's not a problem. There's no reason to
        // fulfill all skipped requests, it's important to send (at least) one ping with the latest
        // info.
        MCH_SIGNAL.signal(Instant::now());
        mch_ticker.next().await;
    }
}
