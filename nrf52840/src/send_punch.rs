//! This module handles sending punches and other data to the server.
//! It uses a BG77 modem and MQTT to communicate with the server.

use crate::error::Error;
use crate::flash::NrfFlash;
use crate::system_info::MCH_SIGNAL;
use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_futures::select::{Either3, select3};
use embassy_nrf::gpio::Output;
use embassy_nrf::uarte::{UarteRxWithIdle, UarteTx};
use embassy_sync::mutex::Mutex;
use embassy_sync::semaphore::{FairSemaphore, Semaphore};
use embassy_time::{Duration, Instant, Timer, WithTimeout};
use yaroc_common::at::response::{FLASH_LOG_CHANNEL, FlashLog};
use yaroc_common::at::uart::AtUart;
use yaroc_common::bg77::modem::Bg77;
use yaroc_common::bg77::mqtt::MQTT_RETRY_COUNT;
use yaroc_common::flash::Flash;
use yaroc_common::{
    RawMutex,
    backoff::{BackoffRetries, PUNCH_QUEUE_SIZE, PunchMsg, SendPunchFn},
    send_punch::{COMMAND_CHANNEL, SendPunch},
};

/// Type alias for the BG77 modem instance.
pub type Bg77Type = Bg77<AtUart<UarteTx<'static>, UarteRxWithIdle<'static>>, Output<'static>>;

/// A type alias for the `SendPunch` struct, configured for the BG77 modem.
pub type Bg77SendPunchType = SendPunch<Bg77Type, NrfFlash<'static>>;

/// A mutex for the flash memory.
pub static FLASH_MUTEX: Mutex<RawMutex, Option<NrfFlash<'static>>> = Mutex::new(None);

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

    /// Returns the timeout for sending a punch.
    pub fn send_punch_timeout(&self) -> Duration {
        self.packet_timeout * (u32::from(MQTT_RETRY_COUNT) + 1)
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
        let mut send_punch_mutex = SEND_PUNCH_MUTEX
            .lock()
            .with_timeout(self.packet_timeout)
            .await
            .map_err(|_| Error::TimeoutError)?;
        let send_punch = send_punch_mutex.as_mut().unwrap();
        send_punch.send_punch_impl(&punch.punches, punch.msg_id).await
    }

    async fn acquire(&mut self) -> crate::Result<Self::SemaphoreReleaser> {
        // The modem doesn't like too many messages being sent out at the same time.
        self.bg77_punch_semaphore.acquire(1).await.map_err(|_| Error::SemaphoreError)
    }

    fn spawn(self, msg: PunchMsg, spawner: Spawner) {
        spawner.spawn(
            bg77_send_punch_fn(msg, self, self.send_punch_timeout()).expect("Failed to spawn task"),
        );
    }
}

/// A task that runs the backoff retries loop.
#[embassy_executor::task]
pub async fn backoff_retries_loop(mut backoff_retries: BackoffRetries<Bg77SendPunchFn>) {
    backoff_retries.r#loop().await;
}

/// A task that logs AT responses and MiniCallHome messages to flash without locking `SEND_PUNCH_MUTEX`.
#[embassy_executor::task]
pub async fn flash_log_task() {
    loop {
        let item = FLASH_LOG_CHANNEL.receive().await;
        let mut flash = FLASH_MUTEX.lock().await;
        if let Some(flash) = flash.as_mut() {
            match item {
                FlashLog::AtResponse(pending_response) => {
                    let _ = flash
                        .log_at_response(pending_response)
                        .await
                        .inspect_err(|e| error!("Failed to log AT response: {}", e));
                }
                FlashLog::MiniCallHome(mini_call_home) => {
                    let _ = flash
                        .log_minicallhome(mini_call_home)
                        .await
                        .inspect_err(|e| error!("Failed to log MiniCallHome: {}", e));
                }
            }
        }
    }
}

/// Main event handler for the `SendPunch` struct.
///
/// This task listens for events from `MCH_SIGNAL` and `COMMAND_CHANNEL` and
/// dispatches them to the `SendPunch` instance.
#[embassy_executor::task]
pub async fn send_punch_event_handler() {
    {
        let mut send_punch_unlocked = SEND_PUNCH_MUTEX.lock().await;
        let send_punch = send_punch_unlocked.as_mut().unwrap();
        let _ = send_punch
            .setup()
            .await
            .inspect_err(|err| error!("Modem setup failed: {}", err));
    }

    let mut next_network_check = Instant::now() + Duration::from_secs(600);
    loop {
        let signal = select3(
            MCH_SIGNAL.wait(),
            COMMAND_CHANNEL.receive(),
            Timer::at(next_network_check),
        )
        .await;
        {
            let mut send_punch_unlocked = SEND_PUNCH_MUTEX.lock().await;
            let send_punch = send_punch_unlocked.as_mut().unwrap();
            match signal {
                Either3::First(_) => match send_punch.send_mini_call_home().await {
                    Ok(_mini_call_home) => info!("MiniCallHome sent"),
                    Err(err) => error!("Sending of MiniCallHome failed: {}", err),
                },
                Either3::Second(command) => send_punch.execute_command(command).await,
                Either3::Third(_) => {
                    next_network_check = Instant::now() + Duration::from_secs(600);
                    send_punch.check_connection().await;
                }
            }
        }
    }
}
