use defmt::{error, warn};
use embassy_futures::select::{select3, Either3};
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer};
use femtopb::repeated;
use heapless::binary_heap::{BinaryHeap, Min};
use yaroc_common::{
    proto::{Punch, Punches},
    punch::RawPunch,
    RawMutex,
};

use crate::mqtt::{MqttPubStatus, MqttPublishReport};

pub const MQTT_MESSAGES: usize = 8;

pub static PUNCHES_TO_SEND: Channel<RawMutex, RawPunch, MQTT_MESSAGES> = Channel::new();
pub static QMTPUB_URCS: Channel<RawMutex, MqttPublishReport, MQTT_MESSAGES> = Channel::new();

#[derive(Clone, Copy, Eq, PartialEq)]
struct PunchMsg {
    punch: RawPunch,
    next_send: Instant,
    backoff: Duration,
    id: Option<u8>,
}

impl Default for PunchMsg {
    fn default() -> Self {
        Self {
            punch: RawPunch::default(),
            next_send: Instant::now() + Duration::from_secs(30),
            backoff: Duration::from_secs(60),
            id: None,
        }
    }
}

impl PunchMsg {
    pub fn new(punch: RawPunch, msg_id: u8) -> Self {
        Self {
            punch,
            id: Some(msg_id),
            ..Default::default()
        }
    }

    pub fn update_next_send(&mut self) {
        self.next_send = Instant::now() + self.backoff;
        self.backoff *= 2; // TODO: configurable
    }
}

impl Ord for PunchMsg {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.next_send.cmp(&other.next_send)
    }
}

impl PartialOrd for PunchMsg {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.next_send.cmp(&other.next_send))
    }
}

#[derive(Default)]
pub struct BackoffRetries {
    queue: BinaryHeap<PunchMsg, Min, MQTT_MESSAGES>,
    inflight_msgs: [PunchMsg; MQTT_MESSAGES],
}

impl BackoffRetries {
    pub async fn r#loop(&mut self) {
        loop {
            let top = self.queue.peek();
            let timer = match top {
                None => Timer::after_secs(3600),
                Some(msg) => Timer::at(msg.next_send),
            };

            match select3(PUNCHES_TO_SEND.receive(), QMTPUB_URCS.receive(), timer).await {
                Either3::First(punch) => {
                    let idx = self.inflight_msgs.iter().position(|msg| msg.id.is_none());
                    if let Some(id) = idx {
                        // TODO: id = 0 is special, should we use it?
                        let msg = PunchMsg::new(punch, id as u8);
                        self.inflight_msgs[id] = msg;
                        let _ = self.queue.push(msg);
                    } else {
                        error!("Message queue is full");
                    }
                }
                Either3::Second(qmtpub_urc) => {
                    if matches!(qmtpub_urc.status, MqttPubStatus::Timeout) {
                        let mut msg = self.inflight_msgs[qmtpub_urc.msg_id as usize];
                        if let Some(id) = msg.id {
                            warn!("Message ID={} timed out, trying again", id);
                            msg.update_next_send();
                            let _ = self.queue.push(msg);
                        } else {
                            error!(
                                "Gor URC for a message we don't know about, ID={}",
                                qmtpub_urc.msg_id
                            );
                        }
                    }
                }
                Either3::Third(_) => {
                    if let Some(punch_msg) = self.queue.pop() {
                        let punch = [Punch {
                            raw: &punch_msg.punch,
                            ..Default::default()
                        }];
                        let _punches = Punches {
                            punches: repeated::Repeated::from_slice(&punch),
                            ..Default::default()
                        };
                        //self.send_message::<40>("p", punches, 1).await
                    }
                }
            }
        }
    }
}
