use crate::{
    at::mqtt::{MqttStatus, StatusCode},
    punch::RawPunch,
    RawMutex,
};
#[cfg(feature = "defmt")]
use defmt::{error, warn};
use embassy_executor::Spawner;
use embassy_sync::{channel::Channel, signal::Signal};
use embassy_time::{Duration, Timer};
use heapless::Vec;
#[cfg(not(feature = "defmt"))]
use log::{error, warn};

pub const PUNCH_QUEUE_SIZE: usize = 24;
pub static CMD_FOR_BACKOFF: Channel<RawMutex, BackoffCommand, { PUNCH_QUEUE_SIZE * 2 }> =
    Channel::new();
const BACKOFF_MULTIPLIER: u32 = 2;

pub enum BackoffCommand {
    PublishPunch(RawPunch, u16),
    PunchPublished(u16, u16),
    MqttDisconnected,
    Status(MqttStatus),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
/// Struct holding all necessary information about a punch that will be send and retried if send
/// fails.
pub struct PunchMsg {
    pub punch: RawPunch,
    backoff: Duration,
    id: u16,
    pub msg_id: u16,
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

/// Trait for a send punch function used by `BackoffRetries` to send punches.
pub trait SendPunchFn {
    fn send_punch(
        &mut self,
        punch: &PunchMsg,
    ) -> impl core::future::Future<Output = crate::Result<()>>;

    fn spawn(self, msg: PunchMsg, spawner: Spawner);
}

// TODO: find a better way of instantiating this
static STATUS_UPDATES: [Signal<RawMutex, StatusCode>; PUNCH_QUEUE_SIZE] = [
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
];

/// Exponential backoff retries for sending punches.
#[derive(Default)]
pub struct BackoffRetries<S: SendPunchFn> {
    unpublished_msgs: Vec<bool, PUNCH_QUEUE_SIZE>,
    send_punch_fn: S,
    initial_backoff: Duration,
}

impl<S: SendPunchFn + Copy> BackoffRetries<S> {
    pub fn new(send_punch_fn: S, initial_backoff: Duration, capacity: usize) -> Self {
        let mut unpublished_msgs = Vec::new();
        unpublished_msgs.resize(capacity + 1, false).expect("capacity set too high");
        Self {
            unpublished_msgs,
            send_punch_fn,
            initial_backoff,
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

    fn handle_status(&mut self, status: MqttStatus) {
        STATUS_UPDATES[status.msg_id as usize].signal(status.code);
    }

    async fn mqtt_disconnected(&mut self) {
        for (idx, val) in self.unpublished_msgs.iter().enumerate() {
            if *val {
                STATUS_UPDATES[idx].signal(StatusCode::MqttError);
            }
        }
    }

    /// Main loop handling the retries.
    ///
    /// Needs to run on a separate thread.
    pub async fn r#loop(&mut self) {
        loop {
            match CMD_FOR_BACKOFF.receive().await {
                BackoffCommand::PublishPunch(punch, punch_id) => {
                    match self.vacant_idx() {
                        // We skip the first element corresponding to ID=0
                        Some(msg_id) if msg_id > 0 => {
                            let msg =
                                PunchMsg::new(punch, punch_id, msg_id as u16, self.initial_backoff);
                            self.unpublished_msgs[msg_id] = true;
                            // Spawn an future that will try to send the punch.
                            self.send_punch_fn.spawn(msg, Spawner::for_current_executor().await);
                        }
                        _ => {
                            error!("Message queue is full");
                        }
                    }
                }
                BackoffCommand::Status(status) => self.handle_status(status),
                BackoffCommand::MqttDisconnected => self.mqtt_disconnected().await,
                BackoffCommand::PunchPublished(_punch_id, msg_id) => {
                    self.delete_msg(msg_id);
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
        STATUS_UPDATES[msg_idx].reset();
        let punch_id = punch_msg.id;

        let res = send_punch_fn.send_punch(&punch_msg).await;
        if res.is_err() {
            STATUS_UPDATES[msg_idx].signal(StatusCode::MqttError);
        }
        loop {
            match STATUS_UPDATES[msg_idx].wait().await {
                StatusCode::Published => {
                    CMD_FOR_BACKOFF
                        .send(BackoffCommand::PunchPublished(punch_id, punch_msg.msg_id))
                        .await;
                    break;
                }
                StatusCode::Timeout | StatusCode::MqttError => {
                    error!(
                        "Punch ID={} failed to send, trying again after {} s",
                        punch_id,
                        punch_msg.backoff.as_secs()
                    );
                    Timer::after(punch_msg.backoff).await;
                    punch_msg.update_backoff();
                    // Updates during backoff are uninteresting, it's probably MQTT disconnect
                    STATUS_UPDATES[msg_idx].reset();
                }
                StatusCode::Retrying(retries) => {
                    warn!(
                        "Sending punch ID={} will be retried, has been tried {} times",
                        punch_id, retries
                    );
                    continue;
                }
                StatusCode::Unknown => {
                    error!("Uknown message status");
                }
            }
            let res = send_punch_fn.send_punch(&punch_msg).await;
            if res.is_err() {
                STATUS_UPDATES[msg_idx].signal(StatusCode::MqttError);
            }
        }
    }
}
