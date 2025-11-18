//! This module handles sending punches and other data to the server.
//! It uses a BG77 modem and MQTT to communicate with the server.

use crate::error::Error;
use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{Either3, select3};
use embassy_nrf::uarte::{UarteRxWithIdle, UarteTx};
use embassy_sync::{
    channel::{Channel, Receiver, Sender},
    semaphore::{FairSemaphore, Semaphore},
};
use embassy_sync::{mutex::Mutex, signal::Signal};
use embassy_time::{Duration, Instant, Ticker, WithTimeout};
use femtopb::{Message, repeated};
use heapless::{Vec, format};
use yaroc_common::{
    PUNCH_EXTRA_LEN, RawMutex,
    at::uart::UrcHandlerType,
    backoff::{
        BackoffRetries, BatchedPunches, PUNCH_BATCH_SIZE, PUNCH_QUEUE_SIZE, PunchMsg, SendPunchFn,
    },
    bg77::{
        hw::{ACTIVATION_TIMEOUT, Bg77, ModemHw},
        mqtt::{MqttClient, MqttConfig, MqttQos},
        system_info::SystemInfo,
    },
    proto::Punches,
    punch::SiPunch,
    send_punch::SendPunchCommand,
    si_uart::SiUart,
};

/// A type alias for the `SendPunch` struct, configured for the BG77 modem.
pub type Bg77SendPunchType = SendPunch<Bg77<UarteTx<'static>, UarteRxWithIdle<'static>>>;

/// A channel for sending `Command`s to the `send_punch_event_handler`.
pub static COMMAND_CHANNEL: Channel<RawMutex, SendPunchCommand, 10> = Channel::new();

/// A signal used to trigger a MiniCallHome event.
static MCH_SIGNAL: Signal<RawMutex, Instant> = Signal::new();

/// A mutex for the `SendPunch` struct.
pub static SEND_PUNCH_MUTEX: Mutex<RawMutex, Option<Bg77SendPunchType>> = Mutex::new(None);
// Property of the Quectel BG77 hardware. Any more than 5 messages inflight fail to send.
const PUNCHES_INFLIGHT: usize = 5;
static BG77_PUNCH_SEMAPHORE: FairSemaphore<RawMutex, PUNCH_QUEUE_SIZE> =
    FairSemaphore::new(PUNCHES_INFLIGHT);

/// A task that runs the backoff retries loop.
#[embassy_executor::task]
pub async fn backoff_retries_loop(mut backoff_retries: BackoffRetries<Bg77SendPunchFn>) {
    backoff_retries.r#loop().await;
}

/// A function that sends a punch using the BG77 modem.
#[derive(Clone, Copy)]
pub struct Bg77SendPunchFn {
    bg77_punch_semaphore: &'static FairSemaphore<RawMutex, PUNCH_QUEUE_SIZE>,
    packet_timeout: Duration,
}

impl Bg77SendPunchFn {
    /// Creates a new `Bg77SendPunchFn`.
    pub fn new(packet_timeout: Duration) -> Self {
        Self {
            bg77_punch_semaphore: &BG77_PUNCH_SEMAPHORE,
            packet_timeout,
        }
    }

    pub fn send_punch_timeout(&self) -> Duration {
        ACTIVATION_TIMEOUT + self.packet_timeout * 2
    }
}

/// A task that sends a punch using the BG77 modem.
#[embassy_executor::task(pool_size = PUNCH_QUEUE_SIZE)]
async fn bg77_send_punch_fn(
    msg: PunchMsg,
    send_punch_fn: Bg77SendPunchFn,
    send_punch_timeout: Duration,
) {
    BackoffRetries::<Bg77SendPunchFn>::try_sending_with_retries(
        msg,
        send_punch_fn,
        send_punch_timeout,
    )
    .await
}

impl SendPunchFn for Bg77SendPunchFn {
    type SemaphoreReleaser = embassy_sync::semaphore::SemaphoreReleaser<
        'static,
        FairSemaphore<RawMutex, PUNCH_QUEUE_SIZE>,
    >;

    async fn send_punch(&mut self, punch: &PunchMsg) -> crate::Result<()> {
        let mut send_punch = SEND_PUNCH_MUTEX
            .lock()
            // TODO: We avoid deadlock by adding a timeout, there might be better solutions
            .with_timeout(self.packet_timeout)
            .await
            .map_err(|_| Error::TimeoutError)?;
        send_punch.as_mut().unwrap().send_punch_impl(&punch.punches, punch.msg_id).await
    }

    async fn acquire(&mut self) -> crate::Result<Self::SemaphoreReleaser> {
        // The modem doesn't like too many messages being sent out at the same time.
        self.bg77_punch_semaphore.acquire(1).await.map_err(|_| Error::SemaphoreError)
    }

    fn spawn(self, msg: PunchMsg, spawner: Spawner) {
        spawner.must_spawn(bg77_send_punch_fn(msg, self, self.send_punch_timeout()));
    }
}

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
    /// * `spawner`: The embassy spawner.
    /// * `mqtt_config`: The MQTT configuration.
    pub fn new(mut bg77: M, spawner: Spawner, mqtt_config: MqttConfig) -> Self {
        let client = MqttClient::new(mqtt_config, 0);
        let handlers: Vec<UrcHandlerType, 3> = Vec::from_array([|response| {
            MqttClient::<M>::urc_handler::<0>(response, COMMAND_CHANNEL.sender())
        }]);
        bg77.spawn(spawner, handlers);
        Self {
            bg77,
            client,
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
        const PROTO_LEN: usize = (yaroc_common::punch::LEN + PUNCH_EXTRA_LEN) * PUNCH_BATCH_SIZE;
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
        match si_uart.read_grouped_punches().await {
            Err(err) => punch_sender.send(Err(err)).await,
            Ok(grouped_punches) => {
                for punches in grouped_punches {
                    punch_sender.send(Ok(punches)).await;
                }
            }
        }
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
    send_punch_mutex: &'static Mutex<RawMutex, Option<Bg77SendPunchType>>,
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
            COMMAND_CHANNEL.receive(),
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
                        COMMAND_CHANNEL
                            .send(SendPunchCommand::MqttConnect(false, Instant::now()))
                            .await;
                        error!("Sending of MiniCallHome failed: {}", err);
                    }
                },
                Either3::Second(command) => send_punch.execute_command(command).await,
                Either3::Third(punch) => send_punch.schedule_punch(punch).await,
            }
        }
    }
}
