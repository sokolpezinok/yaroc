// Note: this test is not finished yet, there's no asserts and it takes too long
use embassy_executor::{Executor, Spawner};
use embassy_futures::select::select;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer};
use heapless::{binary_heap::Min, BinaryHeap};
use static_cell::StaticCell;
use yaroc_common::{
    at::mqtt::{MqttStatus, StatusCode},
    backoff::{BackoffCommands, BackoffRetries, SendPunchFn, CMD_FOR_BACKOFF, PUNCH_QUEUE_SIZE},
    punch::RawPunch,
    RawMutex,
};

#[derive(Eq, PartialEq)]
struct TimedResponse {
    time: Instant,
    status: MqttStatus,
}

impl TimedResponse {
    pub fn new(time: Instant, status: MqttStatus) -> Self {
        Self { time, status }
    }
}

impl PartialOrd for TimedResponse {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.time.partial_cmp(&other.time)
    }
}

impl Ord for TimedResponse {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.time.cmp(&other.time)
    }
}

static TIMED_RESPONSES: Channel<RawMutex, TimedResponse, PUNCH_QUEUE_SIZE> = Channel::new();
static PUBLISH_EVENTS: Channel<RawMutex, (u16, Instant), PUNCH_QUEUE_SIZE> = Channel::new();

#[derive(Default)]
struct FakeSendPunchFn {
    counters: [u8; 10],
    send_timeout: Duration,
    successful_send: Duration,
}

impl FakeSendPunchFn {
    pub fn new(send_timeout: Duration, successful_send: Duration) -> Self {
        Self {
            counters: Default::default(),
            send_timeout,
            successful_send,
        }
    }
}

impl FakeSendPunchFn {
    pub async fn process_responses(
        timed_responses: &'static Channel<RawMutex, TimedResponse, PUNCH_QUEUE_SIZE>,
    ) {
        let mut queue = BinaryHeap::<TimedResponse, Min, PUNCH_QUEUE_SIZE>::new();

        loop {
            let timer = match queue.peek() {
                None => Timer::after_secs(3600),
                Some(msg) => Timer::at(msg.time),
            };
            match select(timer, timed_responses.receive()).await {
                embassy_futures::select::Either::First(_) => {
                    let top = queue.pop().unwrap();
                    if top.status.code == StatusCode::Published {
                        PUBLISH_EVENTS.send((top.status.msg_id, Instant::now())).await;
                    }
                    // TODO: this notification should come from BackoffRetries
                    CMD_FOR_BACKOFF.send(BackoffCommands::Status(top.status)).await;
                }
                embassy_futures::select::Either::Second(timed_response) => {
                    let _ = queue.push(timed_response);
                }
            }
        }
    }
}

impl SendPunchFn for FakeSendPunchFn {
    async fn send_punch(&mut self, punch: RawPunch, msg_id: u16) -> yaroc_common::Result<()> {
        let cnt = punch[0];
        let msg_idx = msg_id as usize;
        let (time, report) = if self.counters[msg_idx] == 0 {
            // First attempt fails
            let report = MqttStatus::mqtt_error(msg_id);
            self.counters[msg_idx] += 1;
            (Instant::now() + self.send_timeout, report)
        } else if self.counters[msg_idx] < cnt {
            // Next attempts time out
            let report = MqttStatus::from_bg77_qmtpub(msg_id, 2, None);
            self.counters[msg_idx] += 1;
            (Instant::now() + self.send_timeout, report)
        } else {
            let report = MqttStatus::from_bg77_qmtpub(msg_id, 0, None);
            let send_time = Instant::now() + self.successful_send;
            (send_time, report)
        };
        TIMED_RESPONSES.send(TimedResponse::new(time, report)).await;
        Ok(())
    }
}

static EXECUTOR: StaticCell<Executor> = StaticCell::new();

#[test]
fn backoff_test() {
    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner| {
        spawner.must_spawn(main(spawner));
    });
}

#[embassy_executor::task]
async fn fake_responder(
    timed_responses: &'static Channel<RawMutex, TimedResponse, PUNCH_QUEUE_SIZE>,
) {
    FakeSendPunchFn::process_responses(&timed_responses).await;
}

#[embassy_executor::task]
async fn backoff_loop(mut backoff: BackoffRetries<FakeSendPunchFn>) {
    backoff.r#loop().await;
}

#[embassy_executor::task]
async fn main(spawner: Spawner) {
    let fake: FakeSendPunchFn =
        FakeSendPunchFn::new(Duration::from_millis(400), Duration::from_millis(200));
    let backoff = BackoffRetries::new(fake, Duration::from_millis(100), 2);
    spawner.must_spawn(backoff_loop(backoff));
    spawner.must_spawn(fake_responder(&TIMED_RESPONSES));

    let mut punch1 = RawPunch::default();
    punch1[0] = 3;
    CMD_FOR_BACKOFF.send(BackoffCommands::PublishPunch(punch1, 0)).await;
    let mut punch2 = RawPunch::default();
    punch2[0] = 2;
    CMD_FOR_BACKOFF.send(BackoffCommands::PublishPunch(punch2, 1)).await;

    let mut punch3 = RawPunch::default();
    punch3[0] = 1;
    CMD_FOR_BACKOFF.send(BackoffCommands::PublishPunch(punch3, 2)).await;

    for _ in 0..2 {
        let (msg_id, time) = PUBLISH_EVENTS.receive().await;
        match msg_id {
            1 => assert!(time.as_millis().abs_diff(1300) <= 10),
            2 => assert!(time.as_millis().abs_diff(2100) <= 15),
            _ => assert!(false, "Got wrong message"),
        }
    }
    assert!(PUBLISH_EVENTS.is_empty());
    std::process::exit(0); // Exit from executor
}
