use crate::{
    bg77_hw::{Bg77, ModemHw},
    error::Error,
    mqtt::{MqttClient, MqttConfig, MqttQos, ACTIVATION_TIMEOUT},
    system_info::{NrfTemp, SystemInfo, Temp},
};
use defmt::{debug, error, info, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_nrf::{
    gpio::Output,
    peripherals::{TIMER0, UARTE1},
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
    Bg77<UarteTx<'static, UARTE1>, UarteRxWithIdle<'static, UARTE1, TIMER0>, Output<'static>>,
    NrfTemp,
>;
pub type SendPunchMutexType = Mutex<RawMutex, Option<SendPunchType>>;

// MiniCallHome signal
static MCH_SIGNAL: Signal<RawMutex, Instant> = Signal::new();

pub enum Command {
    SynchronizeTime,
    MqttConnect(bool, Instant),
}
pub static EVENT_CHANNEL: Channel<RawMutex, Command, 10> = Channel::new();

pub struct SendPunch<M: ModemHw, T: Temp> {
    bg77: M,
    client: MqttClient<M>,
    system_info: SystemInfo<M, T>,
    last_reconnect: Option<Instant>,
}

impl<M: ModemHw, T: Temp> SendPunch<M, T> {
    pub fn new(
        mut bg77: M,
        temp: T,
        send_punch_mutex: &'static SendPunchMutexType,
        spawner: &Spawner,
        config: MqttConfig,
    ) -> Self {
        bg77.spawn(MqttClient::<M>::urc_handler, spawner);
        Self {
            bg77,
            client: MqttClient::new(send_punch_mutex, config, spawner),
            system_info: SystemInfo::<M, T>::new(temp),
            last_reconnect: None,
        }
    }

    // TODO: this method probably doesn't belong here
    pub async fn config(&mut self) -> Result<(), Error> {
        self.bg77.simple_call_at("E0", None).await?;
        let cmd = format!(100; "+CGDCONT=1,\"IP\",\"{}\"", "trial-nbiot.corp")?;
        let _ = self.bg77.simple_call_at(&cmd, None).await;
        self.bg77.simple_call_at("+CEREG=2", None).await?;
        self.bg77.simple_call_at("+QCFG=\"nwscanseq\",03", None).await?;
        self.bg77.simple_call_at("+QCFG=\"iotopmode\",1,1", None).await?;
        self.bg77.simple_call_at("+QCFG=\"band\",0,0,80000", None).await?;
        self.bg77.call_at("+CGATT=1", ACTIVATION_TIMEOUT).await?;
        Ok(())
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
        let res = self.client.send_message(&mut self.bg77, topic, &buf[..len], qos, msg_id).await;
        if res.is_err() {
            EVENT_CHANNEL.send(Command::MqttConnect(false, Instant::now())).await;
        }
        res
    }

    pub async fn send_mini_call_home(&mut self) -> crate::Result<()> {
        let mini_call_home =
            self.system_info.mini_call_home(&mut self.bg77).await.ok_or(Error::ModemError)?;
        self.send_message::<250>("status", mini_call_home.to_proto(), MqttQos::Q0, 0)
            .await
    }

    /// Schedules the SI punch to be handled by `BackoffRetries`.
    pub async fn schedule_punch(&mut self, punch: crate::Result<SiPunch>) {
        match punch {
            Ok(punch) => {
                let id = self.client.schedule_punch(punch.raw).await;
                info!(
                    "{} punched {} at {}, ID={}",
                    punch.card,
                    punch.code,
                    format!(30; "{}", punch.time).unwrap().as_str()[..23],
                    id,
                );
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
        self.config().await?;

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
            Command::MqttConnect(force, origin) => {
                debug!(
                    "MQTT connect request took {} milliseconds to arrive",
                    (Instant::now() - origin).as_millis()
                );
                if !force
                    && self.last_reconnect.map(|t| t + Duration::from_secs(30) > Instant::now())
                        == Some(true)
                {
                    return;
                }

                if let Err(err) = self.mqtt_connect().await {
                    error!("Error connecting to MQTT: {}", err);
                }
                self.last_reconnect = Some(Instant::now());
            }
            Command::SynchronizeTime => {
                let time = self.synchronize_time().await;
                match time {
                    None => warn!("Cannot get modem time"),
                    Some(time) => {
                        info!("Modem time: {}", format!(30; "{}", time).unwrap().as_str())
                    }
                }
            }
        }
    }
}

#[embassy_executor::task]
pub async fn send_punch_main_loop(send_punch_mutex: &'static SendPunchMutexType) {
    {
        let mut send_punch_unlocked = send_punch_mutex.lock().await;
        let send_punch = send_punch_unlocked.as_mut().unwrap();
        if let Err(err) = send_punch.setup().await {
            error!("Setup failed: {}", err);
        }
    }

    let mut mch_ticker = Ticker::every(Duration::from_secs(20));
    let mut get_time_ticker = Ticker::every(Duration::from_secs(300));
    loop {
        match select(mch_ticker.next(), get_time_ticker.next()).await {
            Either::First(_) => MCH_SIGNAL.signal(Instant::now()),
            Either::Second(_) => EVENT_CHANNEL.send(Command::SynchronizeTime).await,
        }
    }
}

#[embassy_executor::task]
pub async fn send_punch_event_handler(
    send_punch_mutex: &'static SendPunchMutexType,
    punch_receiver: Receiver<'static, RawMutex, Result<SiPunch, Error>, 5>,
) {
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
                    Err(err) => error!("Sending of MiniCallHome failed: {}", err),
                },
                Either3::Second(command) => send_punch.execute_command(command).await,
                Either3::Third(punch) => send_punch.schedule_punch(punch).await,
            }
        }
    }
}
