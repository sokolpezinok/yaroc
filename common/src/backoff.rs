use crate::{
    at::mqtt::{MqttStatus, StatusCode},
    punch::RawPunch,
    RawMutex,
};
#[cfg(feature = "defmt")]
use defmt::{error, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_sync::{
    channel::Channel,
    lazy_lock::LazyLock,
    pubsub::{ImmediatePublisher, PubSubChannel, Subscriber},
    signal::Signal,
};
use embassy_time::{Duration, Instant, Timer};
use heapless::Vec;
#[cfg(not(feature = "defmt"))]
use log::{error, warn};

pub trait Random {
    fn u16(&mut self) -> impl core::future::Future<Output = u16>;
}

pub const PUNCH_QUEUE_SIZE: usize = 24;
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
/// Struct holding all necessary information about a punch that will be send and retried if send
/// fails.
pub struct PunchMsg {
    pub punch: RawPunch,
    backoff: Duration,
    pub id: u16,
    pub msg_id: u16,
    pub jitter_ms: u16,
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
            jitter_ms: 0,
        }
    }
}

impl PunchMsg {
    pub fn new(
        punch: RawPunch,
        id: u16,
        msg_id: u16,
        initial_backoff: Duration,
        jitter_ms: u16,
    ) -> Self {
        Self {
            punch,
            id,
            msg_id, // TODO: can't be 0
            backoff: initial_backoff,
            jitter_ms,
        }
    }

    pub fn update_backoff(&mut self) {
        self.backoff *= BACKOFF_MULTIPLIER;
    }
}

/// Trait for a send punch function used by `BackoffRetries` to send punches.
pub trait SendPunchFn {
    type SemaphoreReleaser;

    /// Acquire concurrent access to the send punch function.
    ///
    /// The releaser should be dropped once you received an publish, timeout or error response via
    /// `STATUS_UPDATES`.
    fn acquire(
        &mut self,
    ) -> impl core::future::Future<Output = crate::Result<Self::SemaphoreReleaser>>;

    /// Send punch. The result of the operation is received via `STATUS_UPDATES`.
    fn send_punch(
        &mut self,
        punch: &PunchMsg,
    ) -> impl core::future::Future<Output = crate::Result<()>>;

    fn spawn(self, msg: PunchMsg, spawner: Spawner);
}

// TODO: find a better way of instantiating this
static STATUS_UPDATES: LazyLock<[Signal<RawMutex, StatusCode>; PUNCH_QUEUE_SIZE]> =
    LazyLock::new(Default::default);

#[derive(Copy, Clone)]
enum MqttEvent {
    Connect,
    Disconnect,
}
static MQTT_EVENTS: PubSubChannel<RawMutex, MqttEvent, 1, PUNCH_QUEUE_SIZE, 1> =
    PubSubChannel::new();

/// Exponential backoff retries for sending punches.
pub struct BackoffRetries<S: SendPunchFn, R: Random> {
    unpublished_msgs: Vec<bool, PUNCH_QUEUE_SIZE>,
    send_punch_fn: S,
    initial_backoff: Duration,
    mqtt_events: ImmediatePublisher<'static, RawMutex, MqttEvent, 1, PUNCH_QUEUE_SIZE, 1>,
    rng: R,
}

impl<S: SendPunchFn + Copy, R: Random> BackoffRetries<S, R> {
    pub fn new(send_punch_fn: S, rng: R, initial_backoff: Duration, capacity: usize) -> Self {
        let mut unpublished_msgs = Vec::new();
        unpublished_msgs.resize(capacity + 1, false).expect("capacity set too high");
        let mqtt_events = MQTT_EVENTS.immediate_publisher();
        Self {
            unpublished_msgs,
            send_punch_fn,
            initial_backoff,
            mqtt_events,
            rng,
        }
    }

    /// Find vacant index in `unpublished_msgs`.
    ///
    /// It's a position set to false;
    fn vacant_idx(&self) -> Option<usize> {
        self.unpublished_msgs.iter().rposition(|val| !val)
    }

    /// Delete message from unpublished messages.
    ///
    /// Typically done after it's been succesfully published.
    fn delete_msg(&mut self, idx: u16) {
        // The `vacant_idx` function will consider this spot empty.
        self.unpublished_msgs[idx as usize] = false;
    }

    async fn handle_publish_request(&mut self, punch: RawPunch, punch_id: u16) {
        match self.vacant_idx() {
            // We skip the first element corresponding to ID=0
            Some(msg_id) if msg_id > 0 => {
                let jitter_ms = self.rng.u16().await % 30_000;
                let msg = PunchMsg::new(
                    punch,
                    punch_id,
                    msg_id as u16,
                    self.initial_backoff,
                    jitter_ms,
                );
                self.unpublished_msgs[msg_id] = true;
                // Spawn an future that will try to send the punch.
                self.send_punch_fn.spawn(msg, Spawner::for_current_executor().await);
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

    /// Main loop handling the retries.
    ///
    /// Needs to run on a separate thread.
    pub async fn r#loop(&mut self) {
        loop {
            match CMD_FOR_BACKOFF.receive().await {
                BackoffCommand::PublishPunch(punch, punch_id) => {
                    self.handle_publish_request(punch, punch_id).await
                }
                BackoffCommand::Status(status) => self.handle_status(status),
                BackoffCommand::MqttDisconnected => self.mqtt_disconnected(),
                BackoffCommand::MqttConnected => self.mqtt_connected(),
                BackoffCommand::PunchPublished(_punch_id, msg_id) => {
                    self.delete_msg(msg_id);
                }
            }
        }
    }

    /// Figure out if message has been sent.
    async fn is_message_sent(
        punch_msg: &mut PunchMsg,
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
                    CMD_FOR_BACKOFF
                        .send(BackoffCommand::PunchPublished(punch_id, punch_msg.msg_id))
                        .await;
                    return true;
                }
                Either::First(StatusCode::Timeout | StatusCode::MqttError)
                | Either::Second(MqttEvent::Disconnect) => {
                    error!(
                        "Punch ID={} failed to send, trying again after {} s",
                        punch_id,
                        punch_msg.backoff.as_millis() as f32 / 1_000.0
                    );

                    // TODO: factor out into a separate function
                    let next_send = punch_msg.next_send();
                    loop {
                        match select(Timer::at(next_send), mqtt_events.next_message_pure()).await {
                            Either::First(_) => {
                                punch_msg.update_backoff();
                                break;
                            }
                            Either::Second(MqttEvent::Connect) => {
                                punch_msg.halve_backoff();
                                // After MQTT connect we sleep for a random time in order to not
                                // overload the MQTT client (modem).
                                let jitter_after_connect =
                                    Duration::from_millis(u64::from(punch_msg.jitter_ms));
                                Timer::after(jitter_after_connect).await;
                                break;
                            }
                            _ => {}
                        }
                    }
                    return false;
                }
                Either::Second(MqttEvent::Connect) => {}
                Either::First(StatusCode::Retrying(retries)) => {
                    warn!(
                        "Sending punch ID={} will be retried, has been tried {} times",
                        punch_id, retries
                    );
                }
                Either::First(StatusCode::Unknown) => {
                    error!("Uknown message status");
                }
            }
        }
    }

    /// Try sending a punch using `send_punch_fn`, retrying if necessary.
    ///
    /// This function is to be used by SendPunchFn::spawn(). We can't spawn it directly, as
    /// embassy_executor::task doesn't allow generic functions and S is a generic parameter.
    pub async fn try_sending_with_retries(mut punch_msg: PunchMsg, mut send_punch_fn: S) {
        // TODO: set expiration deadline
        let msg_idx = punch_msg.msg_id as usize;
        STATUS_UPDATES.get()[msg_idx].reset();
        let mut mqtt_events = MQTT_EVENTS.subscriber().unwrap();

        loop {
            let _releaser = send_punch_fn.acquire().await.unwrap();
            let res = send_punch_fn.send_punch(&punch_msg).await;
            if res.is_err() {
                STATUS_UPDATES.get()[msg_idx].signal(StatusCode::MqttError);
            }
            if Self::is_message_sent(&mut punch_msg, &mut mqtt_events).await {
                break;
            }
        }
    }
}
