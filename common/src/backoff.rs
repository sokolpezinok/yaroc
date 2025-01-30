use crate::{
    at::mqtt::{MqttStatus, StatusCode},
    punch::RawPunch,
    RawMutex,
};
#[cfg(feature = "defmt")]
use defmt::{error, info, warn};
use embassy_futures::select::{select, Either};
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer};
use heapless::{
    binary_heap::{BinaryHeap, Min},
    Vec,
};
#[cfg(not(feature = "defmt"))]
use log::{error, info, warn};

pub const PUNCH_QUEUE_SIZE: usize = 8;
pub static CMD_FOR_BACKOFF: Channel<RawMutex, BackoffCommands, { PUNCH_QUEUE_SIZE * 2 }> =
    Channel::new();
const BACKOFF_MULTIPLIER: u32 = 2;

pub enum BackoffCommands {
    PublishPunch(RawPunch, u32),
    Status(MqttStatus),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
/// Struct holding all necessary information about a punch that will be send and retried if send
/// fails.
pub struct PunchMsg {
    next_send: Instant,
    punch: RawPunch,
    backoff: Duration,
    id: u32,
    msg_id: u16,
}

impl Default for PunchMsg {
    fn default() -> Self {
        Self {
            punch: RawPunch::default(),
            next_send: Instant::now(),
            backoff: Duration::from_secs(1),
            id: 0,
            msg_id: 0,
        }
    }
}

impl PunchMsg {
    pub fn new(punch: RawPunch, id: u32, msg_id: u16, initial_backoff: Duration) -> Self {
        Self {
            punch,
            id,
            msg_id, // TODO: can't be 0
            backoff: initial_backoff,
            next_send: Instant::now(),
        }
    }

    pub fn update_next_send(&mut self) {
        self.next_send = Instant::now() + self.backoff;
        self.backoff *= BACKOFF_MULTIPLIER;
    }
}

/// Trait for a send punch function used by `BackoffRetries` to send punches.
pub trait SendPunchFn {
    fn send_punch(
        &mut self,
        punch: RawPunch,
        msg_id: u16,
    ) -> impl core::future::Future<Output = crate::Result<()>>;
}

/// Exponential backoff retries for sending punches.
#[derive(Default)]
pub struct BackoffRetries<S: SendPunchFn> {
    queue: BinaryHeap<PunchMsg, Min, PUNCH_QUEUE_SIZE>,
    inflight_msgs: Vec<PunchMsg, PUNCH_QUEUE_SIZE>,
    send_punch_fn: S,
    initial_backoff: Duration,
}

impl<S: SendPunchFn> BackoffRetries<S> {
    pub fn new(send_punch_impl: S, initial_backoff: Duration, capacity: usize) -> Self {
        let mut inflight_msgs = Vec::new();
        inflight_msgs
            .resize(capacity + 1, PunchMsg::default())
            .expect("capacity set too high");
        Self {
            queue: Default::default(),
            inflight_msgs,
            send_punch_fn: send_punch_impl,
            initial_backoff,
        }
    }

    /// Find vacant index in `inflight_msgs`.
    ///
    /// It's a position with PunchMsg.id == 0.
    fn vacant_idx(&self) -> Option<usize> {
        self.inflight_msgs.iter().rposition(|msg| msg.msg_id == 0)
    }

    /// Delete message from infligh messages.
    ///
    /// Typically done after it's been succesfully sent.
    fn delete_msg(&mut self, idx: u16) {
        // Setting ID to 0 is the deletion operation. The `vacant_idx` function will consider this
        // spot empty.
        self.inflight_msgs[idx as usize].msg_id = 0;
    }

    async fn handle_status(&mut self, status: MqttStatus) {
        match status.code {
            StatusCode::Timeout | StatusCode::MqttError => {
                let msg = &mut self.inflight_msgs[status.msg_id as usize];
                if msg.msg_id > 0 {
                    warn!("Message ID={} failed to send, trying again", msg.msg_id);
                    msg.update_next_send();
                    self.queue.push(*msg).expect("Not enough space in queue");
                } else {
                    error!(
                        "Gor URC for a message we don't know about, ID={}",
                        status.msg_id
                    );
                }
            }
            StatusCode::Published => {
                self.delete_msg(status.msg_id);
                info!("Message published");
            }
            StatusCode::Retrying(retries) => {
                warn!("Message will be retried, has been tried {} times", retries);
            }
            _ => {
                error!("Uknown message status");
            }
        }
    }

    /// Main loop handling the retries.
    ///
    /// Needs to run on a separate thread.
    pub async fn r#loop(&mut self) {
        loop {
            let top = self.queue.peek();
            let timer = match top {
                None => Timer::after_secs(3600),
                Some(msg) => Timer::at(msg.next_send),
            };

            match select(CMD_FOR_BACKOFF.receive(), timer).await {
                Either::First(BackoffCommands::PublishPunch(punch, punch_id)) => {
                    match self.vacant_idx() {
                        // We skip the first element corresponding to ID=0
                        Some(msg_id) if msg_id > 0 => {
                            let msg =
                                PunchMsg::new(punch, punch_id, msg_id as u16, self.initial_backoff);
                            self.inflight_msgs[msg_id] = msg;
                            self.queue
                                .push(msg)
                                .expect("Queue should have space if 'inflight_msgs' has space");
                        }
                        _ => {
                            error!("Message queue is full");
                        }
                    }
                }
                Either::First(BackoffCommands::Status(status)) => self.handle_status(status).await,
                Either::Second(_) => {
                    if let Some(punch_msg) = self.queue.pop() {
                        let msg_id = punch_msg.msg_id;
                        if msg_id == 0 {
                            continue;
                        }
                        if let Err(err) =
                            self.send_punch_fn.send_punch(punch_msg.punch, msg_id).await
                        {
                            error!("Error while sending punch ID={}: {}", punch_msg.id, err);
                            let status = MqttStatus::mqtt_error(msg_id);
                            CMD_FOR_BACKOFF.send(BackoffCommands::Status(status)).await;
                        } else {
                            // TODO: succesfully sent punch ID=punch_msg.id. Should send
                            // notification
                        }
                    }
                }
            }
        }
    }
}
