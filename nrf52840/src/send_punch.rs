use crate::{
    bg77_hw::{Bg77, ModemHw},
    error::Error,
    mqtt::{MqttClient, MqttConfig, MqttQos},
    system_info::SystemInfo,
};
use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{select3, Either3};
use embassy_nrf::{
    gpio::Output,
    peripherals::{TIMER1, UARTE1},
    uarte::{UarteRxWithIdle, UarteTx},
};
use embassy_sync::{channel::Channel, mutex::Mutex};
use embassy_sync::{channel::Receiver, signal::Signal};
use embassy_time::{Duration, Instant, Ticker};
use femtopb::{repeated, Message};
use heapless::format;
use yaroc_common::{
    proto::{Punch, Punches},
    punch::{RawPunch, SiPunch},
    RawMutex,
};

pub type SendPunchType = SendPunch<
    Bg77<UarteTx<'static, UARTE1>, UarteRxWithIdle<'static, UARTE1, TIMER1>, Output<'static>>,
>;
pub type SendPunchMutexType = Mutex<RawMutex, Option<SendPunchType>>;

// MiniCallHome signal
static MCH_SIGNAL: Signal<RawMutex, Instant> = Signal::new();

pub enum Command {
    SynchronizeTime,
    MqttConnect(bool, Instant),
    BatteryUpdate,
}
pub static EVENT_CHANNEL: Channel<RawMutex, Command, 10> = Channel::new();

pub struct SendPunch<M: ModemHw> {
    bg77: M,
    client: MqttClient<M>,
    system_info: SystemInfo<M>,
    last_reconnect: Option<Instant>,
}

impl<M: ModemHw> SendPunch<M> {
    pub fn new(
        mut bg77: M,
        send_punch_mutex: &'static SendPunchMutexType,
        spawner: Spawner,
        mqtt_config: MqttConfig,
    ) -> Self {
        bg77.spawn(MqttClient::<M>::urc_handler, spawner);
        Self {
            bg77,
            client: MqttClient::new(send_punch_mutex, mqtt_config, spawner),
            system_info: SystemInfo::<M>::default(),
            last_reconnect: None,
        }
    }

    async fn send_message<const N: usize>(
        &mut self,
        topic: &str,
        msg: impl Message<'_>,
        qos: MqttQos,
        msg_id: u16,
    ) -> Result<(), Error> {
        let mut buf = [0u8; N];
        msg.encode(&mut buf.as_mut_slice()).map_err(|_| Error::BufferTooSmallError)?;
        let len = msg.encoded_len();
        self.client.send_message(&mut self.bg77, topic, &buf[..len], qos, msg_id).await
    }

    pub async fn send_mini_call_home(&mut self) -> crate::Result<()> {
        let mini_call_home =
            self.system_info.mini_call_home(&mut self.bg77).await.ok_or(Error::ModemError)?;
        self.send_message::<250>("status", mini_call_home.to_proto(), MqttQos::Q0, 0)
            .await
    }

    /// Schedules the SI punch to be handled by `BackoffRetries`.
    pub async fn schedule_punch(&mut self, punch: crate::Result<RawPunch>) {
        match punch {
            Ok(punch) => {
                let id = self.client.schedule_punch(punch).await;
                if let Some(time) = self.system_info.current_time(&mut self.bg77, true).await {
                    let today = time.date_naive();
                    let punch = SiPunch::from_raw(punch, today);
                    info!(
                        "{} punched {} at {}, ID={}",
                        punch.card,
                        punch.code,
                        format!(30; "{}", punch.time).unwrap(),
                        id,
                    );
                }
            }
            Err(err) => {
                error!("Wrong punch: {}", err);
            }
        }
    }

    pub async fn send_punch_impl(&mut self, punch: RawPunch, msg_id: u16) -> crate::Result<()> {
        let punch = [Punch {
            raw: &punch,
            ..Default::default()
        }];
        let punches = Punches {
            punches: repeated::Repeated::from_slice(&punch),
            ..Default::default()
        };
        self.send_message::<40>("p", punches, MqttQos::Q1, msg_id).await
    }

    /// Basic setup of the modem
    pub async fn setup(&mut self) -> crate::Result<()> {
        let _ = self.bg77.turn_on().await;
        self.bg77.configure().await?;

        let _ = self.client.mqtt_connect(&mut self.bg77).await;
        Ok(())
    }

    /// Connects to the configured MQTT server.
    async fn mqtt_connect(&mut self) -> crate::Result<()> {
        self.client.mqtt_connect(&mut self.bg77).await
    }

    /// Synchronizes time with the network time of the modem
    async fn synchronize_time(&mut self) -> Option<chrono::DateTime<chrono::FixedOffset>> {
        self.system_info.current_time(&mut self.bg77, false).await
    }

    pub async fn execute_command(&mut self, command: Command) {
        match command {
            Command::MqttConnect(force, _) => {
                if !force
                    && self.last_reconnect.map(|t| t + Duration::from_secs(30) > Instant::now())
                        == Some(true)
                {
                    return;
                }

                let res = self.mqtt_connect().await;
                self.last_reconnect = Some(Instant::now());
                let _ = res.inspect_err(|err| error!("Error connecting to MQTT: {}", err));
            }
            Command::SynchronizeTime => {
                let time = self.synchronize_time().await;
                match time {
                    None => warn!("Cannot get modem time"),
                    Some(time) => {
                        info!("Modem time: {}", format!(30; "{}", time).unwrap())
                    }
                }
            }
            Command::BatteryUpdate => {
                let _ = self
                    .system_info
                    .update_battery_state(&mut self.bg77)
                    .await
                    .inspect_err(|err| error!("Error while getting battery state: {}", err));
            }
        }
    }
}

#[embassy_executor::task]
pub async fn minicallhome_loop(minicallhome_interval: Duration) {
    let mut mch_ticker = Ticker::every(minicallhome_interval);
    loop {
        // We use Signal, so that MiniCallHome requests do not queue up. If we do not fulfill a few
        // requests, e.g. during a long network search, it's not a problem. There's no reason to
        // fulfill all skipped requests, it's important to send (at least) one ping with the latest
        // info.
        MCH_SIGNAL.signal(Instant::now());
        mch_ticker.next().await;
    }
}

#[embassy_executor::task]
pub async fn send_punch_event_handler(
    send_punch_mutex: &'static SendPunchMutexType,
    punch_receiver: Receiver<'static, RawMutex, Result<RawPunch, Error>, 40>,
) {
    {
        let mut send_punch_unlocked = send_punch_mutex.lock().await;
        let send_punch = send_punch_unlocked.as_mut().unwrap();
        if let Err(err) = send_punch.setup().await {
            error!("Setup failed: {}", err);
        }
    }

    loop {
        let signal = select3(
            MCH_SIGNAL.wait(),
            EVENT_CHANNEL.receive(),
            punch_receiver.receive(),
        )
        .await;
        {
            let mut send_punch_unlocked = send_punch_mutex.lock().await;
            let send_punch = send_punch_unlocked.as_mut().unwrap();
            match signal {
                Either3::First(_) => match send_punch.send_mini_call_home().await {
                    Ok(()) => info!("MiniCallHome sent"),
                    Err(err) => {
                        EVENT_CHANNEL.send(Command::MqttConnect(false, Instant::now())).await;
                        error!("Sending of MiniCallHome failed: {}", err);
                    }
                },
                Either3::Second(command) => send_punch.execute_command(command).await,
                Either3::Third(punch) => send_punch.schedule_punch(punch).await,
            }
        }
    }
}
