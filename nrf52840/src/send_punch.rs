//! This module handles sending punches and other data to the server.
//! It uses a BG77 modem and MQTT to communicate with the server.

use crate::{
    error::Error,
    mqtt::{MqttClient, MqttConfig, MqttQos},
};
use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{Either3, select3};
use embassy_nrf::{
    gpio::Output,
    uarte::{UarteRxWithIdle, UarteTx},
};
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::{mutex::Mutex, signal::Signal};
use embassy_time::{Duration, Instant, Ticker};
use femtopb::{Message, repeated};
use heapless::{Vec, format};
use yaroc_common::{
    RawMutex,
    backoff::{BatchedPunches, PUNCH_BATCH_SIZE},
    bg77::{
        hw::{Bg77, ModemHw},
        system_info::SystemInfo,
    },
    proto::{Punch, Punches},
    punch::SiPunch,
    si_uart::SiUart,
};

/// A type alias for the `SendPunch` struct, configured for the BG77 modem.
pub type SendPunchType =
    SendPunch<Bg77<UarteTx<'static>, UarteRxWithIdle<'static>, Output<'static>>>;
/// A type alias for a mutex-guarded `Option<SendPunchType>`.
pub type SendPunchMutexType = Mutex<RawMutex, Option<SendPunchType>>;

/// A signal used to trigger a MiniCallHome event.
static MCH_SIGNAL: Signal<RawMutex, Instant> = Signal::new();

/// Commands to be sent to the `send_punch_event_handler`.
pub enum Command {
    /// Instructs the modem to synchronize its time with the network.
    SynchronizeTime,
    /// Instructs the modem to connect to the MQTT broker.
    ///
    /// The `bool` parameter indicates whether to force a reconnection.
    MqttConnect(bool, Instant),
    /// Instructs the modem to update the battery status.
    BatteryUpdate,
}
/// A channel for sending `Command`s to the `send_punch_event_handler`.
pub static EVENT_CHANNEL: Channel<RawMutex, Command, 10> = Channel::new();

/// A handler for sending punches and other data to the server.
///
/// This struct manages the modem, the MQTT client, and system information.
pub struct SendPunch<M: ModemHw> {
    bg77: M,
    client: MqttClient<M>,
    system_info: SystemInfo<M>,
    last_reconnect: Option<Instant>,
}

impl<M: ModemHw> SendPunch<M> {
    /// Creates a new `SendPunch` instance.
    ///
    /// # Arguments
    ///
    /// * `bg77`: An initialized modem instance.
    /// * `send_punch_mutex`: A mutex to access the `SendPunch` instance.
    /// * `spawner`: The embassy spawner.
    /// * `mqtt_config`: The MQTT configuration.
    pub fn new(
        mut bg77: M,
        send_punch_mutex: &'static SendPunchMutexType,
        spawner: Spawner,
        mqtt_config: MqttConfig,
    ) -> Self {
        bg77.spawn(MqttClient::<M>::urc_handler, spawner);
        Self {
            bg77,
            client: MqttClient::new(send_punch_mutex, mqtt_config, spawner),
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
        let mut punch_protos = Vec::<Punch, PUNCH_BATCH_SIZE>::new();
        for punch in punches {
            let _ = punch_protos.push(Punch {
                raw: punch,
                ..Default::default()
            });
        }

        let punches_proto = Punches {
            punches: repeated::Repeated::from_slice(&punch_protos),
            ..Default::default()
        };
        const PROTO_LEN: usize = (20 + 4) * PUNCH_BATCH_SIZE + 2;
        self.send_message::<PROTO_LEN>("p", punches_proto, MqttQos::Q1, msg_id).await
    }

    /// Performs the basic setup of the modem.
    ///
    /// This function turns on the modem, configures it, and connects to the MQTT broker.
    pub async fn setup(&mut self) -> crate::Result<()> {
        let _ = self.bg77.turn_on().await;
        self.bg77.configure().await?;

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

    /// Executes a `Command`.
    ///
    /// # Arguments
    ///
    /// * `command`: The command to be executed.
    pub async fn execute_command(&mut self, command: Command) {
        match command {
            Command::MqttConnect(force, _) => {
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
            Command::SynchronizeTime => {
                let time = self.synchronize_time().await;
                match time {
                    None => warn!("Cannot get modem time"),
                    Some(time) => {
                        info!("Modem time: {}", format!(40; "{}", time).unwrap())
                    }
                }
            }
            Command::BatteryUpdate => {
                let _ = self
                    .system_info
                    .update_battery_state(&mut self.bg77)
                    .await
                    .inspect_err(|err| error!("Error while getting battery state: {}", err));
            }
        }
    }
}

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

/// A task that reads punches from the SI-UART and sends them to a channel.
///
/// This task is designed to run continuously, reading punches from the `si_uart`
/// and sending them to the `punch_sender` channel. This decouples the reading of
/// punches from their processing, which is important because the processing might
/// involve waiting for the modem, which can be a long operation.
#[embassy_executor::task]
pub async fn read_si_uart(
    mut si_uart: SiUart<UarteRxWithIdle<'static>>,
    punch_sender: Sender<'static, RawMutex, Result<BatchedPunches, Error>, 24>,
) {
    loop {
        // TODO: batch data from si_uart
        punch_sender.send(si_uart.read().await.map(|p| [p].into())).await;
    }
}

/// The main event handler for the `SendPunch` struct.
///
/// This task listens for events from the `MCH_SIGNAL`, `EVENT_CHANNEL`, and `si_uart` and
/// dispatches them to the `SendPunch` instance.
///
/// # Arguments
///
/// * `send_punch_mutex`: A mutex to access the `SendPunch` instance.
/// * `punch_receiver`: The receiver for batched punches.
#[embassy_executor::task]
pub async fn send_punch_event_handler(
    send_punch_mutex: &'static SendPunchMutexType,
    punch_receiver: Receiver<'static, RawMutex, Result<BatchedPunches, Error>, 24>,
) {
    {
        let mut send_punch_unlocked = send_punch_mutex.lock().await;
        let send_punch = send_punch_unlocked.as_mut().unwrap();
        send_punch
            .setup()
            .await
            .inspect_err(|err| error!("Setup failed: {}", err))
            .expect("Setup failed");
    }

    loop {
        let signal = select3(
            MCH_SIGNAL.wait(),
            EVENT_CHANNEL.receive(),
            punch_receiver.receive(),
        )
        .await;
        {
            let mut send_punch_unlocked = send_punch_mutex.lock().await;
            let send_punch = send_punch_unlocked.as_mut().unwrap();
            match signal {
                Either3::First(_) => match send_punch.send_mini_call_home().await {
                    Ok(()) => info!("MiniCallHome sent"),
                    Err(err) => {
                        EVENT_CHANNEL.send(Command::MqttConnect(false, Instant::now())).await;
                        error!("Sending of MiniCallHome failed: {}", err);
                    }
                },
                Either3::Second(command) => send_punch.execute_command(command).await,
                Either3::Third(punch) => send_punch.schedule_punch(punch).await,
            }
        }
    }
}
