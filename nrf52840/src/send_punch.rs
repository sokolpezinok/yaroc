//! This module handles sending punches and other data to the server.
//! It uses a BG77 modem and MQTT to communicate with the server.

use crate::error::Error;
use crate::system_info::MCH_SIGNAL;
use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_futures::select::{Either3, select3};
use embassy_nrf::uarte::{UarteRxWithIdle, UarteTx};
use embassy_sync::mutex::Mutex;
use embassy_sync::{
    channel::Receiver,
    semaphore::{FairSemaphore, Semaphore},
};
use embassy_time::{Duration, Instant, WithTimeout};
use yaroc_common::{
    RawMutex,
    backoff::{BackoffRetries, BatchedPunches, PUNCH_QUEUE_SIZE, PunchMsg, SendPunchFn},
    bg77::hw::{ACTIVATION_TIMEOUT, Bg77},
    send_punch::{COMMAND_CHANNEL, SendPunch, SendPunchCommand},
};

/// A type alias for the `SendPunch` struct, configured for the BG77 modem.
pub type Bg77SendPunchType = SendPunch<Bg77<UarteTx<'static>, UarteRxWithIdle<'static>>>;

/// A mutex for the `SendPunch` struct.
pub static SEND_PUNCH_MUTEX: Mutex<RawMutex, Option<Bg77SendPunchType>> = Mutex::new(None);
// Property of the Quectel BG77 hardware. Any more than 5 messages inflight fail to send.
const PUNCHES_INFLIGHT: usize = 5;
static BG77_PUNCH_SEMAPHORE: FairSemaphore<RawMutex, PUNCH_QUEUE_SIZE> =
    FairSemaphore::new(PUNCHES_INFLIGHT);

/// A function that sends a punch using the BG77 modem.
#[derive(Clone, Copy)]
pub struct Bg77SendPunchFn {
    bg77_punch_semaphore: &'static FairSemaphore<RawMutex, PUNCH_QUEUE_SIZE>,
    packet_timeout: Duration,
}

impl Bg77SendPunchFn {
    /// Creates a new `Bg77SendPunchFn`.
    pub fn new(packet_timeout: Duration) -> Self {
        Self {
            bg77_punch_semaphore: &BG77_PUNCH_SEMAPHORE,
            packet_timeout,
        }
    }

    pub fn send_punch_timeout(&self) -> Duration {
        ACTIVATION_TIMEOUT + self.packet_timeout * 2
    }
}

/// A task that sends a punch using the BG77 modem.
#[embassy_executor::task(pool_size = PUNCH_QUEUE_SIZE)]
async fn bg77_send_punch_fn(
    msg: PunchMsg,
    send_punch_fn: Bg77SendPunchFn,
    send_punch_timeout: Duration,
) {
    BackoffRetries::<Bg77SendPunchFn>::try_sending_with_retries(
        msg,
        send_punch_fn,
        send_punch_timeout,
    )
    .await
}

impl SendPunchFn for Bg77SendPunchFn {
    type SemaphoreReleaser = embassy_sync::semaphore::SemaphoreReleaser<
        'static,
        FairSemaphore<RawMutex, PUNCH_QUEUE_SIZE>,
    >;

    async fn send_punch(&mut self, punch: &PunchMsg) -> crate::Result<()> {
        let mut send_punch = SEND_PUNCH_MUTEX
            .lock()
            // TODO: We avoid deadlock by adding a timeout, there might be better solutions
            .with_timeout(self.packet_timeout)
            .await
            .map_err(|_| Error::TimeoutError)?;
        send_punch.as_mut().unwrap().send_punch_impl(&punch.punches, punch.msg_id).await
    }

    async fn acquire(&mut self) -> crate::Result<Self::SemaphoreReleaser> {
        // The modem doesn't like too many messages being sent out at the same time.
        self.bg77_punch_semaphore.acquire(1).await.map_err(|_| Error::SemaphoreError)
    }

    fn spawn(self, msg: PunchMsg, spawner: Spawner) {
        spawner.must_spawn(bg77_send_punch_fn(msg, self, self.send_punch_timeout()));
    }
}

/// A task that runs the backoff retries loop.
#[embassy_executor::task]
pub async fn backoff_retries_loop(mut backoff_retries: BackoffRetries<Bg77SendPunchFn>) {
    backoff_retries.r#loop().await;
}

/// The main event handler for the `SendPunch` struct.
///
/// This task listens for events from the `MCH_SIGNAL`, `EVENT_CHANNEL`, and `si_uart` and
/// dispatches them to the `SendPunch` instance.
///
/// # Arguments
///
/// * `send_punch_mutex`: A mutex to access the `SendPunch` instance.
/// * `punch_receiver`: The receiver for batched punches.
#[embassy_executor::task]
pub async fn send_punch_event_handler(
    punch_receiver: Receiver<'static, RawMutex, Result<BatchedPunches, Error>, 24>,
) {
    {
        let mut send_punch_unlocked = SEND_PUNCH_MUTEX.lock().await;
        let send_punch = send_punch_unlocked.as_mut().unwrap();
        send_punch
            .setup()
            .await
            .inspect_err(|err| error!("Setup failed: {}", err))
            .expect("Setup failed");
    }

    loop {
        let signal = select3(
            MCH_SIGNAL.wait(),
            COMMAND_CHANNEL.receive(),
            punch_receiver.receive(),
        )
        .await;
        {
            let mut send_punch_unlocked = SEND_PUNCH_MUTEX.lock().await;
            let send_punch = send_punch_unlocked.as_mut().unwrap();
            match signal {
                Either3::First(_) => match send_punch.send_mini_call_home().await {
                    Ok(()) => info!("MiniCallHome sent"),
                    Err(err) => {
                        COMMAND_CHANNEL
                            .send(SendPunchCommand::MqttConnect(false, Instant::now()))
                            .await;
                        error!("Sending of MiniCallHome failed: {}", err);
                    }
                },
                Either3::Second(command) => send_punch.execute_command(command).await,
                Either3::Third(punch) => send_punch.schedule_punch(punch).await,
            }
        }
    }
}
