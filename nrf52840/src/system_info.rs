use defmt::{error, info};
use embassy_futures::select::{Either, select};
use embassy_nrf::saadc::Saadc;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Ticker};
use yaroc_common::{
    RawMutex,
    send_punch::{COMMAND_CHANNEL, SendPunchCommand},
    status::{BATTERY, BatteryInfo, TEMPERATURE, Temp, voltage_to_percent},
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
    // Initial commands to get measurements
    COMMAND_CHANNEL.send(SendPunchCommand::SynchronizeTime).await;

    let temp_sender = TEMPERATURE.sender();
    let mut temperature_ticker = Ticker::every(Duration::from_secs(120));
    let mut time_sync_ticker = Ticker::every(Duration::from_secs(1800));
    loop {
        match select(temperature_ticker.next(), time_sync_ticker.next()).await {
            Either::First(_) => {
                let _ = temp
                    .cpu_temperature()
                    .await
                    .map(|t| temp_sender.send(t))
                    .inspect_err(|err| error!("Temperature update failed: {}", err));
            }
            Either::Second(_) => COMMAND_CHANNEL.send(SendPunchCommand::SynchronizeTime).await,
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

/// A task that periodically updates the battery voltage.
///
/// The battery voltage calculation is based on the WisBlock RAK4630 implementation:
/// https://github.com/RAKWireless/WisBlock/blob/master/examples/RAK4630/power/RAK4630_Battery_Level_Detect/Read_Battery_Level/Read_Battery_Level.ino
///
/// Hardware constants:
/// - ADC resolution: 12-bit (0-4095)
/// - ADC reference: 3.0V (using internal reference)
/// - Voltage divider factor: 1.6667 (hardware divider to bring Vbat within ADC range: 1M / (1M + 1.5M))
/// - Multiplier: (3000mV / 4096) * 1.6667 = 0.732421875 * 1.6666666667 ≈ 1.220703125
#[embassy_executor::task]
pub async fn battery_update(mut saadc: Saadc<'static, 1>) {
    saadc.calibrate().await;
    let mut buf = [0; 1];
    let battery_sender = BATTERY.sender();

    // TODO: take the interval as an argument
    let mut ticker = Ticker::every(Duration::from_secs(120));
    loop {
        saadc.sample(&mut buf).await;
        // WisBlock uses a 1.6667 voltage divider and 12-bit ADC with 3.0V reference.
        // The raw value is in buf[0].
        let raw = buf[0].max(0);
        let mv = (f32::from(raw) * 1.2207031) as u16;
        let percents = voltage_to_percent(mv);
        info!("Battery voltage: {} mV", mv);
        battery_sender.send(BatteryInfo { mv, percents });
        ticker.next().await;
    }
}
