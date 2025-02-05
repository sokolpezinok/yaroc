// Note: this test is not finished yet, there's no asserts and it takes too long
use embassy_executor::{Executor, Spawner};
use embassy_sync::{
    channel::Channel,
    pubsub::{PubSubChannel, Subscriber},
};
use embassy_time::{Duration, Instant, Timer, WithTimeout};
use static_cell::StaticCell;
use yaroc_common::{
    at::mqtt::{MqttStatus, StatusCode},
    backoff::{BackoffCommand, BackoffRetries, PunchMsg, Random, SendPunchFn, CMD_FOR_BACKOFF},
    punch::RawPunch,
    RawMutex,
};

#[derive(Debug, Eq, PartialEq)]
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

const PUNCH_COUNT: usize = 4;

static COMMANDS: [Channel<RawMutex, TimedResponse, PUNCH_COUNT>; PUNCH_COUNT] = [
    Channel::new(),
    Channel::new(),
    Channel::new(),
    Channel::new(),
];
static MQTT_DISCONNECT: PubSubChannel<RawMutex, bool, 3, PUNCH_COUNT, 1> = PubSubChannel::new();
static PUBLISH_EVENTS: Channel<RawMutex, (u16, Instant), PUNCH_COUNT> = Channel::new();

struct FakeRandom;
impl Random for FakeRandom {
    async fn u16(&mut self) -> u16 {
        0
    }
}

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

#[embassy_executor::task(pool_size = PUNCH_COUNT)]
async fn respond_to_fake(
    punch_id: usize,
    mut mqtt_notifications: Subscriber<'static, RawMutex, bool, 3, PUNCH_COUNT, 1>,
) {
    loop {
        let timed_response = COMMANDS[punch_id].receive().await;
        while mqtt_notifications.try_next_message_pure().is_some() {} // Clear old
        if mqtt_notifications
            .next_message_pure()
            .with_deadline(timed_response.time)
            .await
            .is_err()
        {
            let status_code = timed_response.status.code;
            CMD_FOR_BACKOFF.send(BackoffCommand::Status(timed_response.status)).await;
            // We actually want the deadline: meaning there was no disconnect during that time
            // We perform no acton for MQTT disconnects.
            if status_code == StatusCode::Published {
                // TODO: This should be a notification from BackoffRetries
                PUBLISH_EVENTS.send((punch_id as u16, Instant::now())).await;
                break;
            }
        }
    }
}

#[embassy_executor::task(pool_size = PUNCH_COUNT)]
async fn fake_send_punch_fn(
    msg: PunchMsg,
    send_punch_fn: FakeSendPunchFn,
) {
    BackoffRetries::<FakeSendPunchFn, FakeRandom>::try_sending_with_retries(
        msg,
        send_punch_fn,
    )
    .await
}

impl SendPunchFn for FakeSendPunchFn {
    async fn send_punch(&mut self, punch: &PunchMsg) -> yaroc_common::Result<()> {
        let cnt = punch.punch[0];
        let msg_id = punch.msg_id;
        let (time, status) = if self.counter == 0 {
            // First attempt fails
            let status = MqttStatus::mqtt_error(msg_id);
            self.counter += 1;
            (Instant::now() + self.send_timeout, status)
        } else if self.counter < cnt {
            // Next attempts time out
            let status = MqttStatus::from_bg77_qmtpub(msg_id, 2, None);
            self.counter += 1;
            (Instant::now() + self.send_timeout, status)
        } else {
            let status = MqttStatus::from_bg77_qmtpub(msg_id, 0, None);
            (Instant::now() + self.successful_send, status)
        };
        COMMANDS[punch.id as usize].send(TimedResponse::new(time, status)).await;
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
async fn backoff_loop(mut backoff: BackoffRetries<FakeSendPunchFn, FakeRandom>) {
    backoff.r#loop().await;
}

#[embassy_executor::task]
async fn main(spawner: Spawner) {
    let fake: FakeSendPunchFn =
        FakeSendPunchFn::new(Duration::from_millis(400), Duration::from_millis(200));
    let backoff = BackoffRetries::new(fake, FakeRandom, Duration::from_millis(100), 2);
    spawner.must_spawn(backoff_loop(backoff));

    // First test
    let mut punch0 = RawPunch::default();
    punch0[0] = 3;
    spawner.must_spawn(respond_to_fake(0, MQTT_DISCONNECT.subscriber().unwrap()));
    CMD_FOR_BACKOFF.send(BackoffCommand::PublishPunch(punch0, 0)).await;
    let mut punch1 = RawPunch::default();
    punch1[0] = 2;
    spawner.must_spawn(respond_to_fake(1, MQTT_DISCONNECT.subscriber().unwrap()));
    CMD_FOR_BACKOFF.send(BackoffCommand::PublishPunch(punch1, 1)).await;
    let mut punch2 = RawPunch::default();
    punch2[0] = 1;
    spawner.must_spawn(respond_to_fake(2, MQTT_DISCONNECT.subscriber().unwrap()));
    CMD_FOR_BACKOFF.send(BackoffCommand::PublishPunch(punch2, 2)).await;

    let disconnect_publisher = MQTT_DISCONNECT.publisher().unwrap();
    // MQTT disconnect at 100 milliseconds cuts the first try (timeout) to just 100 ms
    Timer::after_millis(100).await;
    disconnect_publisher.publish_immediate(true);
    CMD_FOR_BACKOFF.send(BackoffCommand::MqttDisconnected).await;

    Timer::after_millis(600).await;
    // MQTT disconnect during a backoff wait should have no effect.
    disconnect_publisher.publish_immediate(true);
    CMD_FOR_BACKOFF.send(BackoffCommand::MqttDisconnected).await;
    Timer::after_millis(10).await;
    disconnect_publisher.publish_immediate(true);
    CMD_FOR_BACKOFF.send(BackoffCommand::MqttDisconnected).await;

    for _ in 0..2 {
        let (punch_id, time) = PUBLISH_EVENTS.receive().await;
        match punch_id {
            // Try 1 is shortened by MQTT disconnect message, so it takes 200 instead of 400 ms.
            // 100 try 1,  400 try 2 + 3, 200 try 4, and in-between (1 + 2 + 4) * 100 backoff
            0 => assert!(time.as_millis().abs_diff(1800) <= 15),
            // 100 try 1,  400 try 2, 200 try 3, and in-between (1 + 2) * 100 backoff
            1 => assert!(time.as_millis().abs_diff(1000) <= 10),
            _ => assert!(false, "Got wrong message"),
        }
    }
    assert!(PUBLISH_EVENTS.is_empty());
    // End of first test

    // Second test
    let start = Instant::now();
    let mut punch3 = RawPunch::default();
    punch3[0] = 3;
    spawner.must_spawn(respond_to_fake(3, MQTT_DISCONNECT.subscriber().unwrap()));
    CMD_FOR_BACKOFF.send(BackoffCommand::PublishPunch(punch3, 3)).await;

    Timer::after_millis(300).await;
    // MQTT connect during sending has no effect
    CMD_FOR_BACKOFF.send(BackoffCommand::MqttConnected).await;
    Timer::after_millis(700).await;
    // MQTT connect shortens the wait
    CMD_FOR_BACKOFF.send(BackoffCommand::MqttConnected).await;

    let (punch_id, time) = PUBLISH_EVENTS.receive().await;
    assert_eq!(punch_id, 3);
    assert!((time - start).as_millis().abs_diff(1700) <= 15);
    // End of second test

    assert!(PUBLISH_EVENTS.is_empty());
    std::process::exit(0); // Exit from executor
}
