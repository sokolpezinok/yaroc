// Note: this test is not finished yet, there's no asserts and it takes too long
use embassy_executor::{Executor, Spawner};
use embassy_futures::select::{select, Either};
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer};
use heapless::{binary_heap::Min, BinaryHeap};
use static_cell::StaticCell;
use yaroc_common::{
    at::mqtt::{MqttStatus, StatusCode},
    backoff::{
        BackoffCommand, BackoffRetries, PunchMsg, SendPunchFn, CMD_FOR_BACKOFF, PUNCH_QUEUE_SIZE,
    },
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

enum Command {
    Response(TimedResponse),
    MqttDisconnected,
}

static COMMANDS: Channel<RawMutex, Command, PUNCH_QUEUE_SIZE> = Channel::new();
static PUBLISH_EVENTS: Channel<RawMutex, (u16, Instant), PUNCH_QUEUE_SIZE> = Channel::new();

#[derive(Clone, Copy, Default)]
struct FakeSendPunchFn {
    counter: u8,
    send_timeout: Duration,
    successful_send: Duration,
}

impl FakeSendPunchFn {
    pub fn new(send_timeout: Duration, successful_send: Duration) -> Self {
        Self {
            counter: 0,
            send_timeout,
            successful_send,
        }
    }
}

impl FakeSendPunchFn {
    pub async fn process_responses(
        commands: &'static Channel<RawMutex, Command, PUNCH_QUEUE_SIZE>,
    ) {
        let mut queue = BinaryHeap::<TimedResponse, Min, PUNCH_QUEUE_SIZE>::new();
        loop {
            let timer = match queue.peek() {
                None => Timer::after_secs(3600),
                Some(msg) => Timer::at(msg.time),
            };
            match select(timer, commands.receive()).await {
                Either::First(_) => {
                    let top = queue.pop().unwrap();
                    if top.status.code == StatusCode::Published {
                        PUBLISH_EVENTS.send((top.status.msg_id, Instant::now())).await;
                    }
                    // TODO: this notification should come from BackoffRetries
                    CMD_FOR_BACKOFF.send(BackoffCommand::Status(top.status)).await;
                }
                Either::Second(Command::Response(timed_response)) => {
                    // TODO: this should panic
                    let _ = queue.push(timed_response);
                }
                Either::Second(Command::MqttDisconnected) => {
                    queue.clear();
                }
            }
        }
    }
}

#[embassy_executor::task(pool_size = PUNCH_QUEUE_SIZE)]
async fn fake_send_punch_fn(msg: PunchMsg, send_punch_fn: FakeSendPunchFn) {
    BackoffRetries::try_sending_with_retries(msg, send_punch_fn).await
}

impl SendPunchFn for FakeSendPunchFn {
    async fn send_punch(&mut self, punch: RawPunch, msg_id: u16) -> yaroc_common::Result<()> {
        let cnt = punch[0];
        let (time, report) = if self.counter == 0 {
            // First attempt fails
            let report = MqttStatus::mqtt_error(msg_id);
            self.counter += 1;
            (Instant::now() + self.send_timeout, report)
        } else if self.counter < cnt {
            // Next attempts time out
            let report = MqttStatus::from_bg77_qmtpub(msg_id, 2, None);
            self.counter += 1;
            (Instant::now() + self.send_timeout, report)
        } else {
            let report = MqttStatus::from_bg77_qmtpub(msg_id, 0, None);
            let send_time = Instant::now() + self.successful_send;
            (send_time, report)
        };
        COMMANDS.send(Command::Response(TimedResponse::new(time, report))).await;
        Ok(())
    }

    fn spawn(self, msg: PunchMsg, spawner: Spawner) {
        spawner.must_spawn(fake_send_punch_fn(msg, self));
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
async fn fake_responder(commands: &'static Channel<RawMutex, Command, PUNCH_QUEUE_SIZE>) {
    FakeSendPunchFn::process_responses(&commands).await;
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
    spawner.must_spawn(fake_responder(&COMMANDS));

    let mut punch1 = RawPunch::default();
    punch1[0] = 3;
    CMD_FOR_BACKOFF.send(BackoffCommand::PublishPunch(punch1, 0)).await;
    let mut punch2 = RawPunch::default();
    punch2[0] = 2;
    CMD_FOR_BACKOFF.send(BackoffCommand::PublishPunch(punch2, 1)).await;
    let mut punch3 = RawPunch::default();
    punch3[0] = 1;
    CMD_FOR_BACKOFF.send(BackoffCommand::PublishPunch(punch3, 2)).await;

    Timer::after_millis(200).await;
    // MQTT disconnect at 200 milliseconds cuts the first timeout in half
    COMMANDS.send(Command::MqttDisconnected).await;
    CMD_FOR_BACKOFF.send(BackoffCommand::MqttDisconnected).await;

    for _ in 0..2 {
        let (msg_id, time) = PUBLISH_EVENTS.receive().await;
        match msg_id {
            // 200 until disconnect + 400 timeout, 200 sending, (1 + 2) * 100 backoff
            1 => assert!(time.as_millis().abs_diff(1100) <= 10),
            // 200 until disconnect + 2 * 400 timeout, 200 sending, (1 + 2 + 4) * 100 backoff
            2 => assert!(time.as_millis().abs_diff(1900) <= 15),
            _ => assert!(false, "Got wrong message"),
        }
    }
    assert!(PUBLISH_EVENTS.is_empty());
    std::process::exit(0); // Exit from executor
}
