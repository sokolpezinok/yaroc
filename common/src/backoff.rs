use crate::{
    RawMutex,
    at::mqtt::{MqttStatus, StatusCode},
    punch::RawPunch,
};
#[cfg(feature = "defmt")]
use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::{
    channel::Channel,
    lazy_lock::LazyLock,
    pubsub::{ImmediatePublisher, PubSubChannel, Subscriber},
    signal::Signal,
};
use embassy_time::{Duration, Instant, Timer, WithTimeout};
use heapless::Vec;
#[cfg(not(feature = "defmt"))]
use log::{error, info, warn};

pub const PUNCH_QUEUE_SIZE: usize = 100;
pub static CMD_FOR_BACKOFF: Channel<RawMutex, BackoffCommand, { PUNCH_QUEUE_SIZE * 2 }> =
    Channel::new();
const BACKOFF_MULTIPLIER: u32 = 2;

pub enum BackoffCommand {
    PublishPunch(RawPunch, u16),
    PunchPublished(u16, u16),
    MqttDisconnected,
    MqttConnected,
    Status(MqttStatus),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
/// A message containing a punch that is to be sent, with retry logic.
pub struct PunchMsg {
    pub punch: RawPunch,
    backoff: Duration,
    pub id: u16,
    pub msg_id: u16,
}

impl PunchMsg {
    pub fn next_send(&self) -> Instant {
        Instant::now() + self.backoff
    }

    fn halve_backoff(&mut self) {
        self.backoff /= 2;
    }
}

impl Default for PunchMsg {
    fn default() -> Self {
        Self {
            punch: RawPunch::default(),
            backoff: Duration::from_secs(1),
            id: 0,
            msg_id: 0,
        }
    }
}

impl PunchMsg {
    pub fn new(punch: RawPunch, id: u16, msg_id: u16, initial_backoff: Duration) -> Self {
        Self {
            punch,
            id,
            msg_id, // TODO: can't be 0
            backoff: initial_backoff,
        }
    }

    pub fn update_backoff(&mut self) {
        self.backoff *= BACKOFF_MULTIPLIER;
    }
}

/// A trait for sending punches, used by [`BackoffRetries`].
pub trait SendPunchFn {
    type SemaphoreReleaser;

    /// Acquires concurrent access for sending a punch.
    ///
    /// The returned releaser should be dropped after receiving a response (publish, timeout,
    /// or error) via `STATUS_UPDATES`.
    fn acquire(
        &mut self,
    ) -> impl core::future::Future<Output = crate::Result<Self::SemaphoreReleaser>>;

    /// Sends a punch. The result of the operation is received via `STATUS_UPDATES`.
    fn send_punch(
        &mut self,
        punch: &PunchMsg,
    ) -> impl core::future::Future<Output = crate::Result<()>>;

    /// Spawns a task to send a punch message with a total timeout.
    ///
    /// The message is dropped if the timeout is exceeded.
    fn spawn(self, msg: PunchMsg, spawner: Spawner, send_punch_timeout: Duration);
}

fn init_status_updates() -> Vec<Signal<RawMutex, StatusCode>, PUNCH_QUEUE_SIZE> {
    let mut res = Vec::new();
    for _ in 0..PUNCH_QUEUE_SIZE {
        let _ = res.push(Signal::new());
    }
    res
}

static STATUS_UPDATES: LazyLock<Vec<Signal<RawMutex, StatusCode>, PUNCH_QUEUE_SIZE>> =
    LazyLock::new(init_status_updates);

#[derive(Copy, Clone)]
enum MqttEvent {
    Connect,
    Disconnect,
}
static MQTT_EVENTS: PubSubChannel<RawMutex, MqttEvent, 1, PUNCH_QUEUE_SIZE, 1> =
    PubSubChannel::new();

/// Manages exponential backoff retries for sending punches.
pub struct BackoffRetries<S: SendPunchFn> {
    unpublished_msgs: Vec<bool, PUNCH_QUEUE_SIZE>,
    send_punch_fn: S,
    initial_backoff: Duration,
    send_punch_timeout: Duration,
    mqtt_events: ImmediatePublisher<'static, RawMutex, MqttEvent, 1, PUNCH_QUEUE_SIZE, 1>,
    spawner: Spawner,
}

impl<S: SendPunchFn + Copy> BackoffRetries<S> {
    pub fn new(
        send_punch_fn: S,
        initial_backoff: Duration,
        send_punch_timeout: Duration,
        capacity: usize,
        spawner: Spawner,
    ) -> Self {
        let mut unpublished_msgs = Vec::new();
        unpublished_msgs.resize(capacity + 1, false).expect("capacity set too high");
        let mqtt_events = MQTT_EVENTS.immediate_publisher();
        Self {
            unpublished_msgs,
            send_punch_fn,
            initial_backoff,
            send_punch_timeout,
            mqtt_events,
            spawner,
        }
    }

    /// Finds a vacant index in `unpublished_msgs`.
    ///
    /// A vacant index is one that is set to `false`.
    fn vacant_idx(&self) -> Option<usize> {
        self.unpublished_msgs.iter().rposition(|val| !val)
    }

    /// Deletes a message from unpublished messages, typically after it has been successfully
    /// published.
    fn delete_msg(&mut self, idx: u16) {
        // The `vacant_idx` function will consider this spot empty.
        self.unpublished_msgs[idx as usize] = false;
    }

    fn handle_publish_request(&mut self, punch: RawPunch, punch_id: u16) {
        match self.vacant_idx() {
            // We skip the first element corresponding to ID=0
            Some(msg_id) if msg_id > 0 => {
                let msg = PunchMsg::new(punch, punch_id, msg_id as u16, self.initial_backoff);
                self.unpublished_msgs[msg_id] = true;
                // Spawn an future that will try to send the punch.
                self.send_punch_fn.spawn(msg, self.spawner, self.send_punch_timeout);
            }
            _ => {
                error!("Message queue is full");
            }
        }
    }

    fn handle_status(&mut self, status: MqttStatus) {
        STATUS_UPDATES.get()[status.msg_id as usize].signal(status.code);
    }

    fn mqtt_disconnected(&mut self) {
        self.mqtt_events.publish_immediate(MqttEvent::Disconnect)
    }

    fn mqtt_connected(&mut self) {
        self.mqtt_events.publish_immediate(MqttEvent::Connect)
    }

    /// The main loop for handling punch sending and retries.
    ///
    /// This should run in a separate task.
    pub async fn r#loop(&mut self) {
        loop {
            match CMD_FOR_BACKOFF.receive().await {
                BackoffCommand::PublishPunch(punch, punch_id) => {
                    self.handle_publish_request(punch, punch_id)
                }

                BackoffCommand::Status(status) => self.handle_status(status),
                BackoffCommand::MqttDisconnected => self.mqtt_disconnected(),
                BackoffCommand::MqttConnected => self.mqtt_connected(),
                BackoffCommand::PunchPublished(punch_id, msg_id) => {
                    info!("Punch ID={} published", punch_id);
                    self.delete_msg(msg_id);
                }
            }
        }
    }

    /// Determines if a message has been successfully sent by waiting for a status update.
    async fn is_message_sent(
        punch_msg: &PunchMsg,
        mqtt_events: &mut Subscriber<'static, RawMutex, MqttEvent, 1, PUNCH_QUEUE_SIZE, 1>,
    ) -> bool {
        let msg_idx = punch_msg.msg_id as usize;
        let punch_id = punch_msg.id;
        loop {
            match select(
                STATUS_UPDATES.get()[msg_idx].wait(),
                mqtt_events.next_message_pure(),
            )
            .await
            {
                Either::First(StatusCode::Published) => {
                    return true;
                }
                Either::First(StatusCode::Timeout | StatusCode::MqttError)
                | Either::Second(MqttEvent::Disconnect) => {
                    error!(
                        "Punch ID={} failed to send, trying again after {} s",
                        punch_id,
                        punch_msg.backoff.as_millis() as f32 / 1_000.0
                    );

                    return false;
                }
                Either::Second(MqttEvent::Connect) => {}
                Either::First(StatusCode::Retrying(retries)) => {
                    warn!(
                        "Sending punch ID={} will be retried by the modem, has been tried {} times",
                        punch_id, retries
                    );
                }
                Either::First(StatusCode::Unknown) => {
                    error!("Uknown message status");
                }
            }
        }
    }

    /// Waits for the backoff period of a punch message.
    ///
    /// The wait can be interrupted by an MQTT connect event.
    async fn backoff(
        punch_msg: &mut PunchMsg,
        mqtt_events: &mut Subscriber<'static, RawMutex, MqttEvent, 1, PUNCH_QUEUE_SIZE, 1>,
    ) {
        let next_send = punch_msg.next_send();
        loop {
            match select(Timer::at(next_send), mqtt_events.next_message_pure()).await {
                Either::First(_) => {
                    punch_msg.update_backoff();
                    return;
                }
                Either::Second(MqttEvent::Connect) => {
                    // After MQTT connect we
                    punch_msg.halve_backoff();
                    return;
                }
                _ => {}
            }
        }
    }

    /// Tries to send a punch with retries.
    ///
    /// This function is intended to be used by [`SendPunchFn::spawn`].
    pub async fn try_sending_with_retries(
        mut punch_msg: PunchMsg,
        mut send_punch_fn: S,
        send_punch_timeout: Duration,
    ) {
        // TODO: set expiration deadline
        let msg_idx = punch_msg.msg_id as usize;
        let punch_id = punch_msg.id;
        STATUS_UPDATES.get()[msg_idx].reset();
        let mut mqtt_events = MQTT_EVENTS.subscriber().unwrap();

        loop {
            let _releaser = send_punch_fn.acquire().await.unwrap();
            let res = send_punch_fn.send_punch(&punch_msg).await;
            if res.is_err() {
                STATUS_UPDATES.get()[msg_idx].signal(StatusCode::MqttError);
            }
            let res = Self::is_message_sent(&punch_msg, &mut mqtt_events)
                .with_timeout(send_punch_timeout)
                .await;
            match res {
                Ok(true) => {
                    // Published
                    CMD_FOR_BACKOFF
                        .send(BackoffCommand::PunchPublished(punch_id, punch_msg.msg_id))
                        .await;
                    break;
                }
                Err(_) => {
                    error!("Response from modem timed out for punch ID={}", punch_id);
                }
                _ => Self::backoff(&mut punch_msg, &mut mqtt_events).await,
            }
        }
    }
}
