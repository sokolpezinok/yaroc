use chrono::{DateTime, FixedOffset};
#[cfg(feature = "defmt")]
use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embassy_time::Duration;
use femtopb::{Message, repeated};
use heapless::{String, Vec, format};
#[cfg(not(feature = "defmt"))]
use log::{error, info, warn};
use sequential_storage::map::PostcardValue;

use crate::at::response::{AT_RESPONSE_SIZE, FLASH_LOG_CHANNEL, FlashLog};
use crate::at::uart::UrcHandlerType;
use crate::backoff::{BatchedPunches, PUNCH_BATCH_SIZE};
use crate::bg77::connection::{ConnectionEvent, ConnectionSupervisor};
use crate::bg77::modem::Modem;
use crate::bg77::modem_manager::{ModemConfig, ModemManager};
use crate::bg77::mqtt::MqttClient;
use crate::bg77::system_info::SystemInfo;
use crate::error::Error;
use crate::flash::{Flash, FlashGuard, FlashValue, ValueIndex};
use crate::mqtt::{MqttClientConfig, MqttConfig, MqttQos, duration_ms};
use crate::proto::Punches;
use crate::status::MiniCallHome;
use crate::{PUNCH_EXTRA_LEN, RawMutex};

/// Commands to be sent to the `send_punch_event_handler`.
pub enum SendPunchCommand {
    /// Instructs the modem to synchronize its time with the network.
    SynchronizeTime,
    /// Event for the connection supervisor.
    ConnectionSupervisorEvent(ConnectionEvent),
}

/// A channel for sending `Command`s to the `send_punch_event_handler`.
pub static COMMAND_CHANNEL: Channel<RawMutex, SendPunchCommand, 10> = Channel::new();

/// A handler for sending punches and other data to the server.
///
/// This struct manages the modem, the MQTT client, and system information.
pub struct SendPunch<M: Modem + 'static, F: Flash + 'static> {
    modem: M,
    mqtt_client: MqttClient<M>,
    modem_manager: ModemManager<M>,
    connection_supervisor: ConnectionSupervisor<M>,
    system_info: SystemInfo<M>,
    flash_mutex: &'static Mutex<RawMutex, Option<F>>,
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
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
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
    /// * `modem`: An initialized modem instance.
    /// * `flash_mutex`: A static reference to the flash mutex.
    /// * `spawner`: The embassy spawner.
    /// * `mqtt_config`: The MQTT configuration.
    /// * `modem_config`: The Modem configuration.
    pub fn new(
        mut modem: M,
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
        modem.spawn_rx(&handlers, spawner);
        Self {
            modem,
            mqtt_client,
            modem_manager,
            connection_supervisor: ConnectionSupervisor::new(),
            system_info: SystemInfo::<M>::default(),
            flash_mutex,
            name,
        }
    }

    /// Locks the flash mutex and returns a guard providing access to the flash instance.
    async fn lock_flash(&self) -> FlashGuard<'static, F> {
        let guard = self.flash_mutex.lock().await;
        FlashGuard::new(guard)
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
    /// * `modem`: An initialized modem instance.
    /// * `flash_mutex`: A static reference to the flash mutex.
    /// * `mqtt_config`: The MQTT configuration.
    /// * `modem_config`: The Modem configuration.
    #[cfg(test)]
    pub fn new_without_spawning(
        modem: M,
        flash_mutex: &'static Mutex<RawMutex, Option<F>>,
        mqtt_config: MqttClientConfig,
        modem_config: ModemConfig,
    ) -> Self {
        let mqtt_client = MqttClient::<_>::new(mqtt_config, 0);
        let modem_manager = ModemManager::new(modem_config);
        Self {
            modem,
            mqtt_client,
            modem_manager,
            connection_supervisor: ConnectionSupervisor::new(),
            system_info: SystemInfo::<M>::default(),
            flash_mutex,
            name: "test-send-punch".try_into().unwrap(),
        }
    }

    #[cfg(test)]
    fn set_last_connect_attempt(&mut self) {
        self.connection_supervisor
            .set_last_connect_attempt(embassy_time::Instant::now());
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
        topic: &str,
        msg: impl Message<'_>,
        qos: MqttQos,
        msg_id: u16,
    ) -> Result<(), Error> {
        if !self.connection_supervisor.is_connected() {
            return Err(Error::NotConnected);
        }
        let mut buf = [0u8; N];
        msg.encode(&mut buf.as_mut_slice()).map_err(|_| Error::BufferTooSmallError)?;
        let len = msg.encoded_len();
        self.mqtt_client
            .send_message(&mut self.modem, topic, &buf[..len], qos, msg_id)
            .await
            .map_err(From::from)
    }

    /// Sends a `MiniCallHome` message, containing system information.
    pub async fn send_mini_call_home(&mut self) -> crate::Result<MiniCallHome> {
        let mini_call_home = self.system_info.mini_call_home(&mut self.modem).await;
        #[cfg(feature = "defmt")]
        info!("MiniCallHome: {}", mini_call_home);

        if !self.connection_supervisor.is_connected() {
            let _ = self.ensure_connected().await;
        }

        let _ = FLASH_LOG_CHANNEL
            .try_send(FlashLog::MiniCallHome(mini_call_home))
            .inspect_err(|e| error!("Error while sending MiniCallHome for logging: {:?}", e));

        if self.connection_supervisor.is_connected() {
            self.send_message::<250>("status", mini_call_home.to_proto(), MqttQos::Q0, 0)
                .await?;
        } else {
            warn!("MQTT not connected, skipping MiniCallHome publish");
        }

        Ok(mini_call_home)
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
        self.send_message::<PROTO_LEN>("p", punches_proto, MqttQos::Q1, msg_id).await
    }

    /// Performs the basic setup of the modem.
    ///
    /// This function turns on the modem, configures it, and connects to the MQTT broker.
    pub async fn setup(&mut self) -> crate::Result<()> {
        self.modem.turn_on().await?;
        let firmware = self.modem_manager.configure(&mut self.modem).await?;
        info!("Modem firmware version: {}", firmware);

        let _ = self.ensure_connected().await;
        Ok(())
    }

    /// Configures the modem
    ///
    /// Returns the current firmware version.
    pub async fn configure_modem(
        &mut self,
        modem_config: ModemConfig,
    ) -> crate::Result<String<AT_RESPONSE_SIZE>> {
        self.modem_manager.update_config(modem_config);
        self.modem_manager.configure(&mut self.modem).await
    }

    /// Configures the MQTT client
    pub async fn configure_mqtt(&mut self, mqtt_config: MqttConfig) -> crate::Result<()> {
        self.mqtt_client.update_reduced_config(mqtt_config);
        let _ = self
            .connection_supervisor
            .ensure_mqtt_connected(&mut self.modem, &mut self.mqtt_client)
            .await;
        Ok(())
    }

    /// Synchronizes the system time with the network time from the modem.
    async fn synchronize_time(&mut self) -> Option<DateTime<FixedOffset>> {
        SystemInfo::current_time(&mut self.modem, false).await
    }

    /// Ensures connection to the cellular network and the MQTT broker.
    async fn ensure_connected(&mut self) -> crate::Result<()> {
        self.connection_supervisor
            .ensure_connected(
                &mut self.modem,
                &mut self.modem_manager,
                &mut self.mqtt_client,
            )
            .await
    }

    /// Checks status of the cellular network and MQTT broker connection.
    pub async fn check_connection(&mut self) {
        self.connection_supervisor
            .check_state(
                &mut self.modem,
                &mut self.modem_manager,
                &mut self.mqtt_client,
            )
            .await
    }

    /// Executes a `SendPunchCommand`.
    ///
    /// # Arguments
    ///
    /// * `command`: The command to be executed.
    pub async fn execute_command(&mut self, command: SendPunchCommand) {
        match command {
            SendPunchCommand::ConnectionSupervisorEvent(event) => {
                self.connection_supervisor.handle_event(event);
                let _ = self.ensure_connected().await;
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
        at::{fake_modem::FakeModem, response::PendingLoggedAtResponse},
        bg77::{
            connection::ConnectionState, modem::Bg77, modem_manager::FakePin,
            system_info::BOOT_TIME,
        },
        flash::{Flash, FlashValue, LoggedAtResponseIterator, MchIterator},
        status::{BATTERY, TEMPERATURE},
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
            _response: PendingLoggedAtResponse,
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

    static FAKE_FLASH_MUTEX: Mutex<RawMutex, Option<FakeFlash>> = Mutex::new(None);

    static TEST_MUTEX: Mutex<RawMutex, ()> = Mutex::new(());

    #[test]
    fn send_punch_instantiation_test() {
        let _lock = block_on(TEST_MUTEX.lock());
        let fake_modem = FakeModem::new(&[("AT+QLTS=2", "+QLTS: \"2025/11/24,01:40:34+04,0\"")]);
        let fake_pin = FakePin {};
        let modem = Bg77::new(fake_modem, fake_pin);
        let mqtt_config = MqttClientConfig::default();

        block_on(async {
            *(FAKE_FLASH_MUTEX.lock().await) = Some(FakeFlash);
        });
        BOOT_TIME.sender().clear();
        TEMPERATURE.sender().send(27.0);
        BATTERY.sender().send(crate::status::BatteryInfo { mv: 3967 });

        let mut send_punch = SendPunch::new_without_spawning(
            modem,
            &FAKE_FLASH_MUTEX,
            mqtt_config,
            ModemConfig::default(),
        );
        assert_eq!(
            send_punch.connection_supervisor.state(),
            ConnectionState::Disconnected
        );

        let expected_date = DateTime::parse_from_rfc3339("2025-11-24T01:40:34+01:00").unwrap();
        assert_eq!(
            block_on(send_punch.synchronize_time()).unwrap(),
            expected_date
        );
    }

    #[test]
    fn send_mini_call_home_flash_log_test() {
        let _lock = block_on(TEST_MUTEX.lock());
        BOOT_TIME.sender().clear();
        let mut fake_modem = FakeModem::new(&[
            ("AT+QLTS=2", "+QLTS: \"2024/12/24,10:48:23+04,0\""),
            ("AT+QCSQ", "+QCSQ: \"NBIoT\",-107,-134,35,-20"),
            ("AT+QCFG=\"celevel\"", "+QCFG: \"celevel\",1"),
            ("AT+CEREG?", "+CEREG: 2,1,\"2008\",\"2B2078\",9"),
            ("AT+QMTPUB=0,0,0,0,\"yar/deadbeef/status\",34", ""),
        ]);
        fake_modem.add_pure_interactions(&[("+QMTPUB", true, "+QMTPUB: 0,0,0")]);
        let modem = Bg77::new(fake_modem, FakePin {});
        let mqtt_config = MqttClientConfig::default();

        block_on(async {
            *(FAKE_FLASH_MUTEX.lock().await) = Some(FakeFlash);
        });

        TEMPERATURE.sender().send(27.0);
        BATTERY.sender().send(crate::status::BatteryInfo { mv: 3967 });

        let mut send_punch = SendPunch::new_without_spawning(
            modem,
            &FAKE_FLASH_MUTEX,
            mqtt_config,
            ModemConfig::default(),
        );
        send_punch.connection_supervisor.set_state(ConnectionState::MqttConnected);

        FLASH_LOG_CHANNEL.clear();
        let mch = block_on(send_punch.send_mini_call_home()).unwrap();
        let logged_item = FLASH_LOG_CHANNEL.try_receive().unwrap();
        assert_eq!(logged_item, FlashLog::MiniCallHome(mch));
    }

    #[test]
    fn send_mini_call_home_disconnected_test() {
        let _lock = block_on(TEST_MUTEX.lock());
        BOOT_TIME.sender().clear();
        let fake_modem = FakeModem::new(&[
            ("AT+QLTS=2", "+QLTS: \"2024/12/24,10:48:23+04,0\""),
            ("AT+QCSQ", "+QCSQ: \"NBIoT\",-107,-134,35,-20"),
            ("AT+QCFG=\"celevel\"", "+QCFG: \"celevel\",1"),
            ("AT+CEREG?", "+CEREG: 2,1,\"2008\",\"2B2078\",9"),
        ]);
        let modem = Bg77::new(fake_modem, FakePin {});
        let mqtt_config = MqttClientConfig::default();

        block_on(async {
            *(FAKE_FLASH_MUTEX.lock().await) = Some(FakeFlash);
        });

        BATTERY.sender().send(crate::status::BatteryInfo { mv: 3967 });

        let mut send_punch = SendPunch::new_without_spawning(
            modem,
            &FAKE_FLASH_MUTEX,
            mqtt_config,
            ModemConfig::default(),
        );
        send_punch.set_last_connect_attempt();

        FLASH_LOG_CHANNEL.clear();
        let mch = block_on(send_punch.send_mini_call_home()).unwrap();
        let logged_item = FLASH_LOG_CHANNEL.try_receive().unwrap();
        assert_eq!(logged_item, FlashLog::MiniCallHome(mch));
    }

    #[test]
    fn configure_mqtt_test() {
        let _lock = block_on(TEST_MUTEX.lock());
        let fake_modem = FakeModem::new(&[
            ("AT+QMTOPEN?", ""),
            ("AT+QMTCFG=\"timeout\",0,35,2,1", ""),
            ("AT+QMTCFG=\"keepalive\",0,70", ""),
            (
                "AT+QMTCFG=\"will\",0,1,1,0,\"yar/deadbeef/will\",\"test_client\"",
                "",
            ),
            ("AT+QMTOPEN=0,\"new.broker.com\",1883", "+QMTOPEN: 0,0"),
            ("AT+QMTCONN?", "+QMTCONN: 0,1"),
            ("AT+QMTCONN=0,\"test_client\"", "+QMTCONN: 0,0,0"),
        ]);
        let modem = Bg77::new(fake_modem, FakePin {});
        let mqtt_config = MqttClientConfig::default();
        let mut send_punch = SendPunch::new_without_spawning(
            modem,
            &FAKE_FLASH_MUTEX,
            mqtt_config,
            ModemConfig::default(),
        );

        assert_eq!(
            send_punch.connection_supervisor.state(),
            ConnectionState::Disconnected
        );

        let new_config = MqttConfig {
            url: String::try_from("new.broker.com").unwrap(),
            ..Default::default()
        };

        let res = block_on(send_punch.configure_mqtt(new_config.clone()));
        assert!(res.is_ok());
        assert_eq!(
            send_punch.connection_supervisor.state(),
            ConnectionState::MqttConnected
        );
    }
}
