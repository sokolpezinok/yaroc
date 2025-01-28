// Note: this test is not finished yet, there's no asserts and it takes too long
use embassy_executor::{Executor, Spawner};
use embassy_futures::select::select;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer};
use heapless::{binary_heap::Min, BinaryHeap};
use static_cell::StaticCell;
use yaroc_common::{
    at::mqtt::MqttPublishReport,
    backoff::{BackoffRetries, SendPunchFn, PUNCHES_TO_SEND, PUNCH_QUEUE_SIZE, QMTPUB_URCS},
    punch::RawPunch,
    RawMutex,
};

#[derive(Eq, PartialEq, Ord, PartialOrd)]
struct TimedResponse {
    time: Instant,
    report: MqttPublishReport,
}

impl TimedResponse {
    pub fn new(time: Instant, report: MqttPublishReport) -> Self {
        Self { time, report }
    }
}

static MSG_RESPONSES: Channel<RawMutex, TimedResponse, PUNCH_QUEUE_SIZE> = Channel::new();

#[derive(Default)]
struct FakeSendPunchFn {
    counters: [u8; 10],
    pub send_time: [Option<Instant>; 10],
    send_timeout: Duration,
    successful_send: Duration,
}

impl FakeSendPunchFn {
    pub fn new(send_timeout: Duration, successful_send: Duration) -> Self {
        Self {
            counters: Default::default(),
            send_time: Default::default(),
            send_timeout,
            successful_send,
        }
    }
}

impl FakeSendPunchFn {
    pub async fn process_responses(
        msg_responses: &'static Channel<RawMutex, TimedResponse, PUNCH_QUEUE_SIZE>,
    ) {
        let mut queue = BinaryHeap::<TimedResponse, Min, PUNCH_QUEUE_SIZE>::new();

        loop {
            let timer = match queue.peek() {
                None => Timer::after_secs(3600),
                Some(msg) => Timer::at(msg.time),
            };
            match select(timer, msg_responses.receive()).await {
                embassy_futures::select::Either::First(_) => {
                    let top = queue.pop().unwrap();
                    QMTPUB_URCS.send(top.report).await;
                }
                embassy_futures::select::Either::Second(timed_response) => {
                    let _ = queue.push(timed_response);
                }
            }
        }
    }
}

impl SendPunchFn for FakeSendPunchFn {
    async fn send_punch(&mut self, punch: RawPunch, msg_id: u8) -> yaroc_common::Result<()> {
        let cnt = punch[0];
        let msg_id = msg_id as usize;
        if self.counters[msg_id] < cnt {
            self.counters[msg_id] += 1;
            let report = MqttPublishReport::from_bg77_qmtpub(msg_id as u8, 2, None);
            MSG_RESPONSES
                .send(TimedResponse::new(
                    Instant::now() + self.send_timeout,
                    report,
                ))
                .await;
        } else {
            let report = MqttPublishReport::from_bg77_qmtpub(msg_id as u8, 0, None);
            let send_time = Instant::now() + self.successful_send;
            println!("{}: {}", msg_id, send_time.as_millis());
            self.send_time[msg_id] = Some(send_time);

            MSG_RESPONSES.send(TimedResponse::new(send_time, report)).await;
        }
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
    msg_responses: &'static Channel<RawMutex, TimedResponse, PUNCH_QUEUE_SIZE>,
) {
    FakeSendPunchFn::process_responses(&msg_responses).await;
}

#[embassy_executor::task]
async fn backoff_loop(mut backoff: BackoffRetries<FakeSendPunchFn>) {
    backoff.r#loop().await;
}

#[embassy_executor::task]
async fn main(spawner: Spawner) {
    let fake: FakeSendPunchFn =
        FakeSendPunchFn::new(Duration::from_millis(400), Duration::from_millis(200));
    let backoff = BackoffRetries::new(fake, Duration::from_millis(100));
    spawner.must_spawn(backoff_loop(backoff));
    spawner.must_spawn(fake_responder(&MSG_RESPONSES));

    let mut punch1 = RawPunch::default();
    punch1[0] = 3;
    PUNCHES_TO_SEND.send(punch1).await;
    let mut punch2 = RawPunch::default();
    punch2[0] = 2;
    PUNCHES_TO_SEND.send(punch2).await;

    Timer::after_secs(3).await;
    std::process::exit(0); // TODO: this is ugly, is there a better way?
}
