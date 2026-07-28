use chrono::{DateTime, FixedOffset};
use core::ops::{Deref, DerefMut};
#[cfg(feature = "defmt")]
use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::{Mutex, MutexGuard};
use embassy_time::{Duration, Instant};
use femtopb::{Message, repeated};
use heapless::{String, Vec, format};
#[cfg(not(feature = "defmt"))]
use log::{error, info, warn};
use sequential_storage::map::PostcardValue;

use crate::at::response::{AT_COMMAND_SIZE, LoggedAtResponse, PendingLoggedAtResponse};
use crate::at::uart::UrcHandlerType;
use crate::backoff::{BatchedPunches, PUNCH_BATCH_SIZE};
use crate::bg77::modem::Modem;
use crate::bg77::modem_manager::{ModemConfig, ModemManager};
use crate::bg77::mqtt::MqttClient;
use crate::bg77::system_info::SystemInfo;
use crate::error::Error;
use crate::flash::{Flash, FlashValue, ValueIndex};
use crate::mqtt::{MqttClientConfig, MqttConfig, MqttQos, duration_ms};
use crate::proto::Punches;
use crate::punch::SiPunch;
use crate::status::MiniCallHome;
use crate::{PUNCH_EXTRA_LEN, RawMutex};

/// Commands to be sent to the `send_punch_event_handler`.
pub enum SendPunchCommand {
    /// Instructs the modem to synchronize its time with the network.
    SynchronizeTime,
    /// Instructs the modem to connect to the MQTT broker.
    ///
    /// The `bool` parameter indicates whether to force a reconnection.
    MqttConnect(bool, Instant),
    NetworkConnect(Instant),
}

/// A channel for sending `Command`s to the `send_punch_event_handler`.
pub static COMMAND_CHANNEL: Channel<RawMutex, SendPunchCommand, 10> = Channel::new();

/// A guard providing mutable access to the flash instance while holding the mutex lock.
pub struct FlashGuard<'a, F: Flash + 'static> {
    guard: MutexGuard<'a, RawMutex, Option<F>>,
}

impl<'a, F: Flash + 'static> Deref for FlashGuard<'a, F> {
    type Target = F;

    fn deref(&self) -> &Self::Target {
        self.guard.as_ref().expect("Flash not initialized")
    }
}

impl<'a, F: Flash + 'static> DerefMut for FlashGuard<'a, F> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard.as_mut().expect("Flash not initialized")
    }
}

/// A guard providing mutable access to the BG77 modem instance while holding the mutex lock.
pub struct Bg77Guard<'a, M: Modem + 'static> {
    guard: MutexGuard<'a, RawMutex, Option<M>>,
}

impl<'a, M: Modem + 'static> Deref for Bg77Guard<'a, M> {
    type Target = M;

    fn deref(&self) -> &Self::Target {
        self.guard.as_ref().expect("BG77 not initialized")
    }
}

impl<'a, M: Modem + 'static> DerefMut for Bg77Guard<'a, M> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard.as_mut().expect("BG77 not initialized")
    }
}

/// A handler for sending punches and other data to the server.
///
/// This struct manages the modem, the MQTT client, and system information.
pub struct SendPunch<M: Modem + 'static, F: Flash + 'static> {
    mqtt_client: MqttClient<M>,
    modem_manager: ModemManager<M>,
    system_info: SystemInfo<M>,
    bg77_mutex: &'static Mutex<RawMutex, Option<M>>,
    flash_mutex: &'static Mutex<RawMutex, Option<F>>,
    last_reconnect: Option<Instant>,
    name: String<24>,
}

/// UART0 RX pin options.
#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum UartRxPin {
    /// The default SCL (P0.14) pin.
    #[default]
    Scl,
    /// The SDA (P0.13) pin.
    Sda,
    /// The AIN1 (P0.31) pin.
    Ain1,
}

/// Configuration for the device.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DeviceConfig {
    /// The name of the device.
    pub name: String<24>,
    /// MiniCallHome send interval
    #[serde(with = "duration_ms")]
    pub minicallhome_interval: Duration,
    /// The PIN used by SRR RX
    pub srr_rx_pin: UartRxPin,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            name: Default::default(),
            minicallhome_interval: Duration::from_secs(30),
            srr_rx_pin: Default::default(),
        }
    }
}

impl PostcardValue<'_> for DeviceConfig {}

impl FlashValue for DeviceConfig {
    const VALUE_INDEX: ValueIndex = ValueIndex::DeviceConfig;
}

impl<M: Modem + 'static, F: Flash + 'static> SendPunch<M, F> {
    /// Creates a new `SendPunch` instance.
    ///
    /// # Arguments
    ///
    /// * `bg77`: An initialized modem instance.
    /// * `bg77_mutex`: A static reference to the BG77 modem mutex.
    /// * `flash_mutex`: A static reference to the flash mutex.
    /// * `spawner`: The embassy spawner.
    /// * `mqtt_config`: The MQTT configuration.
    /// * `modem_config`: The Modem configuration.
    pub fn new(
        bg77: &mut M,
        bg77_mutex: &'static Mutex<RawMutex, Option<M>>,
        flash_mutex: &'static Mutex<RawMutex, Option<F>>,
        spawner: Spawner,
        mqtt_config: MqttClientConfig,
        modem_config: ModemConfig,
    ) -> Self {
        let name = mqtt_config.name.clone();
        let mqtt_client = MqttClient::<_>::new(mqtt_config, 0);
        let modem_manager = ModemManager::new(modem_config);

        let handlers: [UrcHandlerType; _] = [
            |response| MqttClient::<M>::urc_handler::<0>(response, COMMAND_CHANNEL.sender()),
            |response| ModemManager::<M>::urc_handler(response, COMMAND_CHANNEL.sender()),
        ];
        bg77.spawn_rx(&handlers, spawner);
        Self {
            mqtt_client,
            modem_manager,
            system_info: SystemInfo::<M>::default(),
            bg77_mutex,
            flash_mutex,
            last_reconnect: None,
            name,
        }
    }

    /// Locks the BG77 mutex and returns a guard providing access to the modem instance.
    pub async fn lock_bg77(&self) -> Bg77Guard<'static, M> {
        let guard = self.bg77_mutex.lock().await;
        Bg77Guard { guard }
    }

    /// Locks the flash mutex and returns a guard providing access to the flash instance.
    pub async fn lock_flash(&self) -> FlashGuard<'static, F> {
        let guard = self.flash_mutex.lock().await;
        FlashGuard { guard }
    }

    /// Updates the device configuration in flash.
    pub async fn update_device_config(
        &mut self,
        mut device_config: DeviceConfig,
    ) -> crate::Result<()> {
        device_config.name = self.name.clone();
        self.lock_flash().await.write(device_config).await?;
        info!("Device config written to flash");
        Ok(())
    }

    /// Creates a new `SendPunch` instance without spawning any tasks.
    ///
    /// This is intended for testing purposes where a `Spawner` is not readily available.
    ///
    /// # Arguments
    ///
    /// * `bg77_mutex`: A static reference to the BG77 modem mutex.
    /// * `flash_mutex`: A static reference to the flash mutex.
    /// * `mqtt_config`: The MQTT configuration.
    /// * `modem_config`: The Modem configuration.
    #[cfg(test)]
    pub fn new_without_spawning(
        bg77_mutex: &'static Mutex<RawMutex, Option<M>>,
        flash_mutex: &'static Mutex<RawMutex, Option<F>>,
        mqtt_config: MqttClientConfig,
        modem_config: ModemConfig,
    ) -> Self {
        let mqtt_client = MqttClient::<_>::new(mqtt_config, 0);
        let modem_manager = ModemManager::new(modem_config);
        Self {
            mqtt_client,
            modem_manager,
            system_info: SystemInfo::<M>::default(),
            bg77_mutex,
            flash_mutex,
            last_reconnect: None,
            name: "test-send-punch".try_into().unwrap(),
        }
    }

    /// Encodes and sends a message to the given MQTT topic.
    ///
    /// # Type Parameters
    ///
    /// * `N`: The size of the buffer for the encoded message.
    ///
    /// # Arguments
    ///
    /// * `topic`: The MQTT topic to which the message is sent.
    /// * `msg`: The message to be sent, which must implement `femtopb::Message`.
    /// * `qos`: The MQTT Quality of Service level.
    /// * `msg_id`: The message identifier.
    async fn send_message<const N: usize>(
        &mut self,
        bg77: &mut M,
        topic: &str,
        msg: impl Message<'_>,
        qos: MqttQos,
        msg_id: u16,
    ) -> Result<(), Error> {
        let mut buf = [0u8; N];
        msg.encode(&mut buf.as_mut_slice()).map_err(|_| Error::BufferTooSmallError)?;
        let len = msg.encoded_len();
        self.mqtt_client.send_message(bg77, topic, &buf[..len], qos, msg_id).await
    }

    /// Sends a `MiniCallHome` message, containing system information.
    pub async fn send_mini_call_home(&mut self) -> crate::Result<MiniCallHome> {
        let mini_call_home = {
            let mut bg77 = self.lock_bg77().await;
            let mini_call_home = self.system_info.mini_call_home(&mut *bg77).await;
            self.send_message::<250>(
                &mut *bg77,
                "status",
                mini_call_home.to_proto(),
                MqttQos::Q0,
                0,
            )
            .await?;

            #[cfg(feature = "defmt")]
            info!("MiniCallHome: {}", mini_call_home);
            mini_call_home
        };

        // TODO: add a test for logging to flash
        let _ = self
            .lock_flash()
            .await
            .log_minicallhome(mini_call_home)
            .await
            .inspect_err(|e| error!("Error while logging MiniCallHome: {}", { e }));
        Ok(mini_call_home)
    }

    /// Schedules a batch of punches to be sent.
    ///
    /// This function processes a batch of punches, logs them, and schedules them for sending.
    pub async fn schedule_punch(&mut self, punch: crate::Result<BatchedPunches>) {
        match punch {
            Ok(punches) => {
                let id = self.mqtt_client.schedule_punches(punches.clone()).await;
                let mut bg77 = self.lock_bg77().await;
                let time = self.system_info.current_time(&mut *bg77, true).await;
                if let Some(time) = time {
                    let today = time.date_naive();
                    for punch in punches {
                        let punch = SiPunch::from_raw(punch, today, time.offset());
                        info!(
                            "{} punched {} at {}, ID={}",
                            punch.card,
                            punch.code,
                            format!(40; "{}", punch.time).unwrap(),
                            id,
                        );
                    }
                }
            }
            Err(err) => {
                error!("Wrong punch: {}", err);
            }
        }
    }

    /// Sends a batch of punches to the server.
    ///
    /// # Arguments
    ///
    /// * `punches`: A vector of raw punches to be sent.
    /// * `msg_id`: The message identifier.
    pub async fn send_punch_impl(
        &mut self,
        punches: &BatchedPunches,
        msg_id: u16,
    ) -> crate::Result<()> {
        let mut punch_messages = Vec::<&[u8], PUNCH_BATCH_SIZE>::new();
        for punch in punches {
            let _ = punch_messages.push(punch);
        }

        let punches_proto = Punches {
            punches: repeated::Repeated::from_slice(&punch_messages),
            ..Default::default()
        };
        const PROTO_LEN: usize = (crate::punch::LEN + PUNCH_EXTRA_LEN) * PUNCH_BATCH_SIZE;
        let mut bg77 = self.lock_bg77().await;
        self.send_message::<PROTO_LEN>(&mut *bg77, "p", punches_proto, MqttQos::Q1, msg_id)
            .await
    }

    /// Performs the basic setup of the modem.
    ///
    /// This function turns on the modem, configures it, and connects to the MQTT broker.
    pub async fn setup(&mut self) -> crate::Result<()> {
        let mut bg77 = self.lock_bg77().await;
        bg77.turn_on().await?;
        let firmware = self.modem_manager.configure(&mut *bg77).await?;
        info!("Modem firmware version: {}", firmware);

        let _ = self.mqtt_client.connect(&mut *bg77, &self.modem_manager).await;
        Ok(())
    }

    /// Configures the modem
    ///
    /// Returns the current firmware version.
    pub async fn configure_modem(
        &mut self,
        modem_config: ModemConfig,
    ) -> crate::Result<String<AT_COMMAND_SIZE>> {
        self.lock_flash().await.write(modem_config.clone()).await?;
        info!("Modem config written to flash");
        self.modem_manager.update_config(modem_config);
        let mut bg77 = self.lock_bg77().await;
        self.modem_manager.configure(&mut *bg77).await
    }

    /// Configures the MQTT client
    pub async fn configure_mqtt(&mut self, mqtt_config: MqttConfig) -> crate::Result<()> {
        self.lock_flash().await.write(mqtt_config.clone()).await?;
        info!("MQTT config written to flash");
        self.mqtt_client.update_reduced_config(mqtt_config);
        let mut bg77 = self.lock_bg77().await;
        self.mqtt_client.disconnect(&mut *bg77).await?;
        Ok(())
    }

    /// Store AT response in flash
    pub async fn log_at_response(
        &mut self,
        response: PendingLoggedAtResponse,
    ) -> crate::Result<()> {
        let timestamp = self.time_from_instant(response.instant);
        let logged_response = LoggedAtResponse {
            timestamp,
            response: response.response,
        };
        self.lock_flash().await.log_at_response(logged_response).await
    }

    /// Connects to the MQTT broker.
    pub async fn mqtt_connect(&mut self) -> crate::Result<()> {
        let mut bg77 = self.lock_bg77().await;
        self.mqtt_client.connect(&mut *bg77, &self.modem_manager).await
    }

    /// Synchronizes the system time with the network time from the modem.
    pub async fn synchronize_time(&mut self) -> Option<DateTime<FixedOffset>> {
        let mut bg77 = self.lock_bg77().await;
        self.system_info.current_time(&mut *bg77, false).await
    }

    /// Returns the calendar time corresponding to the given `instant`, if synchronized.
    fn time_from_instant(&self, instant: Instant) -> DateTime<FixedOffset> {
        self.system_info.time_from_instant(instant)
    }

    /// Executes a `SendPunchCommand`.
    ///
    /// # Arguments
    ///
    /// * `command`: The command to be executed.
    pub async fn execute_command(&mut self, command: SendPunchCommand) {
        match command {
            SendPunchCommand::MqttConnect(force, _) => {
                if !force
                    && self
                        .last_reconnect
                        .is_some_and(|t| t + Duration::from_secs(30) > Instant::now())
                {
                    return;
                }

                let res = self.mqtt_connect().await;
                self.last_reconnect = Some(Instant::now());
                let _ = res.inspect_err(|err| error!("Error connecting to MQTT: {}", err));
            }
            SendPunchCommand::NetworkConnect(_) => {
                //TODO: do something with it
            }
            SendPunchCommand::SynchronizeTime => {
                let time = self.synchronize_time().await;
                match time {
                    None => warn!("Cannot get modem time"),
                    Some(time) => {
                        info!("Modem time: {}", format!(40; "{}", time).unwrap())
                    }
                }
            }
        }
    }
}

#[cfg(feature = "std")]
#[cfg(test)]
mod tests {
    use embassy_futures::block_on;

    use crate::{
        at::{
            fake_modem::FakeModem,
            response::{AtResponse, FromModem},
        },
        bg77::{modem::Bg77, modem_manager::FakePin},
        flash::{Flash, FlashValue, LoggedAtResponseIterator, MchIterator},
    };

    use super::*;

    struct FakeFlash;

    impl Flash for FakeFlash {
        async fn erase(&mut self) -> crate::Result<()> {
            Ok(())
        }

        async fn write<V: FlashValue>(&mut self, _value: V) -> crate::Result<()> {
            Ok(())
        }

        async fn log_minicallhome(
            &mut self,
            _mch: crate::status::MiniCallHome,
        ) -> crate::Result<()> {
            Ok(())
        }

        async fn log_at_response(
            &mut self,
            _response: crate::at::response::LoggedAtResponse,
        ) -> crate::Result<()> {
            Ok(())
        }

        async fn read<V: FlashValue>(&mut self) -> crate::Result<Option<V>> {
            Ok(None)
        }

        type MchIter<'a> = FakeMchIter;

        async fn mch_iter(&mut self) -> crate::Result<Self::MchIter<'_>> {
            Ok(FakeMchIter)
        }

        type LoggedAtResponseIter<'a> = FakeLoggedAtResponseIter;

        async fn logged_at_response_iter(
            &mut self,
        ) -> crate::Result<Self::LoggedAtResponseIter<'_>> {
            Ok(FakeLoggedAtResponseIter)
        }
    }

    struct FakeMchIter;

    impl MchIterator for FakeMchIter {
        async fn next<'b>(&'b mut self) -> crate::Result<Option<crate::proto::MiniCallHome<'b>>> {
            Ok(None)
        }
    }

    struct FakeLoggedAtResponseIter;

    impl LoggedAtResponseIterator for FakeLoggedAtResponseIter {
        async fn next(&mut self) -> crate::Result<Option<crate::at::response::LoggedAtResponse>> {
            Ok(None)
        }
    }

    static FAKE_BG77_MUTEX: Mutex<RawMutex, Option<Bg77<FakeModem, FakePin>>> = Mutex::new(None);
    static FAKE_FLASH_MUTEX: Mutex<RawMutex, Option<FakeFlash>> = Mutex::new(None);

    #[test]
    fn send_punch_instantiation_test() {
        let fake_modem = FakeModem::new(&[("AT+QLTS=2", "+QLTS: \"2025/11/24,01:40:34+04,0\"")]);
        let fake_pin = FakePin {};
        let modem = Bg77::new(fake_modem, fake_pin);
        let mqtt_config = MqttClientConfig::default();

        block_on(async {
            *(FAKE_BG77_MUTEX.lock().await) = Some(modem);
            *(FAKE_FLASH_MUTEX.lock().await) = Some(FakeFlash);
        });

        let mut send_punch = SendPunch::new_without_spawning(
            &FAKE_BG77_MUTEX,
            &FAKE_FLASH_MUTEX,
            mqtt_config,
            ModemConfig::default(),
        );
        assert!(send_punch.last_reconnect.is_none());

        // Test logging AT response when time is not synchronized yet (uses Unix 0 timestamp base)
        let response = PendingLoggedAtResponse {
            response: AtResponse::new([FromModem::Ok].into(), "+CSQ"),
            instant: Instant::from_millis(5000),
        };
        assert!(block_on(send_punch.log_at_response(response)).is_ok());

        let expected_date = DateTime::parse_from_rfc3339("2025-11-24T01:40:34+01:00").unwrap();
        assert_eq!(
            block_on(send_punch.synchronize_time()).unwrap(),
            expected_date
        );
    }
}
