#[cfg(feature = "defmt")]
use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant};
use femtopb::{Message, repeated};
use heapless::{Vec, format};
#[cfg(not(feature = "defmt"))]
use log::{error, info, warn};

use crate::at::uart::UrcHandlerType;
use crate::backoff::{BatchedPunches, PUNCH_BATCH_SIZE};
use crate::bg77::hw::ModemHw;
use crate::bg77::modem_manager::{ModemConfig, ModemManager, ModemPin};
use crate::bg77::mqtt::{MqttClient, MqttConfig, MqttQos};
use crate::bg77::system_info::SystemInfo;
use crate::error::Error;
use crate::proto::Punches;
use crate::punch::SiPunch;
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
    /// Instructs the modem to update the battery status.
    BatteryUpdate,
}

/// A channel for sending `Command`s to the `send_punch_event_handler`.
pub static COMMAND_CHANNEL: Channel<RawMutex, SendPunchCommand, 10> = Channel::new();

/// A handler for sending punches and other data to the server.
///
/// This struct manages the modem, the MQTT client, and system information.
pub struct SendPunch<M: ModemHw, P: ModemPin> {
    bg77: M,
    modem_pin: P,
    client: MqttClient<M>,
    modem_manager: ModemManager,
    system_info: SystemInfo<M>,
    last_reconnect: Option<Instant>,
}

impl<M: ModemHw, P: ModemPin> SendPunch<M, P> {
    /// Creates a new `SendPunch` instance.
    ///
    /// # Arguments
    ///
    /// * `bg77`: An initialized modem instance.
    /// * `modem_pin`: The pin used to reset/turn on the modem.
    /// * `spawner`: The embassy spawner.
    /// * `mqtt_config`: The MQTT configuration.
    /// * `modem_config`: The Modem configuration.
    pub fn new(
        mut bg77: M,
        modem_pin: P,
        spawner: Spawner,
        mqtt_config: MqttConfig,
        modem_config: ModemConfig,
    ) -> Self {
        let client = MqttClient::<_>::new(mqtt_config, 0);
        let modem_manager = ModemManager::new(modem_config);

        let handlers: [UrcHandlerType; _] = [
            |response| MqttClient::<M>::urc_handler::<0>(response, COMMAND_CHANNEL.sender()),
            |response| ModemManager::urc_handler(response, COMMAND_CHANNEL.sender()),
        ];
        bg77.spawn_rx(&handlers, spawner);
        Self {
            bg77,
            modem_pin,
            client,
            modem_manager,
            system_info: SystemInfo::<M>::default(),
            last_reconnect: None,
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
        topic: &str,
        msg: impl Message<'_>,
        qos: MqttQos,
        msg_id: u16,
    ) -> Result<(), Error> {
        let mut buf = [0u8; N];
        msg.encode(&mut buf.as_mut_slice()).map_err(|_| Error::BufferTooSmallError)?;
        let len = msg.encoded_len();
        self.client.send_message(&mut self.bg77, topic, &buf[..len], qos, msg_id).await
    }

    /// Sends a `MiniCallHome` message, containing system information.
    pub async fn send_mini_call_home(&mut self) -> crate::Result<()> {
        let mini_call_home =
            self.system_info.mini_call_home(&mut self.bg77).await.ok_or(Error::ModemError)?;
        self.send_message::<250>("status", mini_call_home.to_proto(), MqttQos::Q0, 0)
            .await
    }

    /// Schedules a batch of punches to be sent.
    ///
    /// This function processes a batch of punches, logs them, and schedules them for sending.
    pub async fn schedule_punch(&mut self, punch: crate::Result<BatchedPunches>) {
        match punch {
            Ok(punches) => {
                let id = self.client.schedule_punch(punches.clone()).await;
                if let Some(time) = self.system_info.current_time(&mut self.bg77, true).await {
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
        self.send_message::<PROTO_LEN>("p", punches_proto, MqttQos::Q1, msg_id).await
    }

    /// Performs the basic setup of the modem.
    ///
    /// This function turns on the modem, configures it, and connects to the MQTT broker.
    pub async fn setup(&mut self) -> crate::Result<()> {
        self.modem_manager.turn_on(&mut self.bg77, &mut self.modem_pin).await?;
        self.modem_manager.configure(&mut self.bg77).await?;

        let _ = self.client.mqtt_connect(&mut self.bg77).await;
        Ok(())
    }

    /// Connects to the MQTT broker.
    async fn mqtt_connect(&mut self) -> crate::Result<()> {
        self.client.mqtt_connect(&mut self.bg77).await
    }

    /// Synchronizes the system time with the network time from the modem.
    async fn synchronize_time(&mut self) -> Option<chrono::DateTime<chrono::FixedOffset>> {
        self.system_info.current_time(&mut self.bg77, false).await
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
            SendPunchCommand::BatteryUpdate => {
                let _ = self
                    .system_info
                    .update_battery_state(&mut self.bg77)
                    .await
                    .inspect_err(|err| error!("Error while getting battery state: {}", err));
            }
        }
    }
}
