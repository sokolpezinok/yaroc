use crate::{
    at::mqtt::{MqttPubStatus, MqttPublishReport},
    punch::RawPunch,
    RawMutex,
};
#[cfg(feature = "defmt")]
use defmt::{error, info, warn};
use embassy_futures::select::{select3, Either3};
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer};
use heapless::binary_heap::{BinaryHeap, Min};
#[cfg(not(feature = "defmt"))]
use log::{error, info, warn};

pub const PUNCH_QUEUE_SIZE: usize = 8;
pub static PUNCHES_TO_SEND: Channel<RawMutex, RawPunch, PUNCH_QUEUE_SIZE> = Channel::new();
pub static PUBLISHING_REPORTS: Channel<RawMutex, MqttPublishReport, PUNCH_QUEUE_SIZE> =
    Channel::new();

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub struct PunchMsg {
    next_send: Instant,
    punch: RawPunch,
    backoff: Duration,
    id: u8,
}

impl Default for PunchMsg {
    fn default() -> Self {
        Self {
            punch: RawPunch::default(),
            next_send: Instant::now(),
            backoff: Duration::from_secs(1),
            id: 0,
        }
    }
}

impl PunchMsg {
    pub fn new(punch: RawPunch, msg_id: u8, initial_backoff: Duration) -> Self {
        Self {
            punch,
            id: msg_id, // TODO: can't be 0
            backoff: initial_backoff,
            next_send: Instant::now(),
        }
    }

    pub fn update_next_send(&mut self) {
        self.next_send = Instant::now() + self.backoff;
        self.backoff *= 2; // TODO: configurable
    }
}

/// Trait for a send punch function used by `BackoffRetries` to send punches.
pub trait SendPunchFn {
    fn send_punch(
        &mut self,
        punch: RawPunch,
        msg_id: u8,
    ) -> impl core::future::Future<Output = crate::Result<()>>;
}

/// Exponential backoff retries for sending punches.
#[derive(Default)]
pub struct BackoffRetries<S: SendPunchFn> {
    queue: BinaryHeap<PunchMsg, Min, PUNCH_QUEUE_SIZE>,
    inflight_msgs: [PunchMsg; PUNCH_QUEUE_SIZE],
    send_punch_impl: S,
    initial_backoff: Duration,
}

impl<S: SendPunchFn> BackoffRetries<S> {
    pub fn new(send_punch_impl: S, initial_backoff: Duration) -> Self {
        Self {
            queue: Default::default(),
            inflight_msgs: Default::default(),
            send_punch_impl,
            initial_backoff,
        }
    }

    /// Find vacant index in `inflight_msgs`.
    ///
    /// It's a position with PunchMsg.id == 0.
    fn vacant_idx(&self) -> Option<usize> {
        self.inflight_msgs.iter().rposition(|msg| msg.id == 0)
    }

    /// Delete message from infligh messages.
    ///
    /// Typically done after it's been succesfully sent.
    fn delete_msg(&mut self, idx: u8) {
        // Setting ID to 0 is the deletion operation. The `vacant_idx` function will consider this
        // spot empty.
        self.inflight_msgs[idx as usize].id = 0;
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

            match select3(
                PUNCHES_TO_SEND.receive(),
                PUBLISHING_REPORTS.receive(),
                timer,
            )
            .await
            {
                Either3::First(punch) => {
                    match self.vacant_idx() {
                        // We skip the first element corresponding to ID=0
                        Some(id) if id > 0 => {
                            let msg = PunchMsg::new(punch, id as u8, self.initial_backoff);
                            self.inflight_msgs[id] = msg;
                            let _ = self.queue.push(msg);
                        }
                        _ => {
                            error!("Message queue is full");
                        }
                    }
                }
                Either3::Second(qmtpub_urc) => match qmtpub_urc.status {
                    MqttPubStatus::Timeout | MqttPubStatus::MqttError => {
                        let msg = &mut self.inflight_msgs[qmtpub_urc.msg_id as usize];
                        if msg.id > 0 {
                            warn!("Message ID={} failed to send, trying again", msg.id);
                            msg.update_next_send();
                            let _ = self.queue.push(*msg);
                        } else {
                            error!(
                                "Gor URC for a message we don't know about, ID={}",
                                qmtpub_urc.msg_id
                            );
                        }
                    }
                    MqttPubStatus::Published => {
                        self.delete_msg(qmtpub_urc.msg_id);
                        info!("Message published");
                    }
                    MqttPubStatus::Retrying(retries) => {
                        warn!("Message will be retried, has been tried {} times", retries);
                    }
                    _ => {
                        error!("Uknown message status");
                    }
                },
                Either3::Third(_) => {
                    if let Some(punch_msg) = self.queue.pop() {
                        let msg_id = punch_msg.id;
                        if msg_id == 0 {
                            continue;
                        }
                        if let Err(err) =
                            self.send_punch_impl.send_punch(punch_msg.punch, msg_id).await
                        {
                            error!("Error while sending punch: {}", err);
                            let report = MqttPublishReport::mqtt_error(msg_id);
                            PUBLISHING_REPORTS.send(report).await;
                        }
                    }
                }
            }
        }
    }
}
