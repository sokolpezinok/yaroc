use crate::{
    bg77_hw::{Bg77, ModemHw},
    error::Error,
    mqtt::{MqttClient, MqttConfig, ACTIVATION_TIMEOUT},
    si_uart::SiUartChannelType,
    system_info::{NrfTemp, SystemInfo, Temp},
};
use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_nrf::{
    gpio::Output,
    peripherals::{TIMER0, UARTE1},
    uarte::{UarteRxWithIdle, UarteTx},
};
use embassy_sync::signal::Signal;
use embassy_sync::{channel::Channel, mutex::Mutex};
use embassy_time::{Duration, Instant, Ticker};
use femtopb::{repeated, Message};
use heapless::format;
use yaroc_common::{
    proto::{Punch, Punches},
    punch::SiPunch,
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
    SynchronizeTime(Instant),
    MqttConnect(bool, Instant),
}
pub static EVENT_CHANNEL: Channel<RawMutex, Command, 10> = Channel::new();

pub struct SendPunch<M: ModemHw, T: Temp> {
    bg77: M,
    client: MqttClient<M>,
    system_info: SystemInfo<M, T>,
}

impl<M: ModemHw, T: Temp> SendPunch<M, T> {
    pub fn new(mut bg77: M, temp: T, spawner: &Spawner, config: MqttConfig) -> Self {
        bg77.spawn(MqttClient::<M>::urc_handler, spawner);
        Self {
            bg77,
            client: MqttClient::new(config),
            system_info: SystemInfo::<M, T>::new(temp),
        }
    }

    // TODO: this method probably doesn't belong here
    pub async fn config(&mut self) -> Result<(), Error> {
        self.bg77.simple_call_at("E0", None).await?;
        self.bg77.simple_call_at("+CEREG=2", None).await?;
        self.bg77.call_at("+CGATT=1", ACTIVATION_TIMEOUT).await?;
        // +QCFG needs +CGATT=1 first
        self.bg77.simple_call_at("+QCFG=\"nwscanseq\",03", None).await?;
        self.bg77.simple_call_at("+QCFG=\"iotopmode\",1,1", None).await?;
        self.bg77.simple_call_at("+QCFG=\"band\",0,0,80000", None).await?;
        Ok(())
    }

    async fn send_message<const N: usize>(
        &mut self,
        topic: &str,
        msg: impl Message<'_>,
        qos: u8,
    ) -> Result<(), Error> {
        let mut buf = [0u8; N];
        msg.encode(&mut buf.as_mut_slice()).map_err(|_| Error::BufferTooSmallError)?;
        let len = msg.encoded_len();
        let res = self.client.send_message(&mut self.bg77, topic, &buf[..len], qos).await;
        if res.is_err() {
            EVENT_CHANNEL.send(Command::MqttConnect(false, Instant::now())).await;
        }
        res
    }

    pub async fn send_mini_call_home(&mut self) -> crate::Result<()> {
        let mini_call_home =
            self.system_info.mini_call_home(&mut self.bg77).await.ok_or(Error::ModemError)?;
        self.send_message::<250>("status", mini_call_home.to_proto(), 0).await
    }

    pub async fn send_punch(&mut self, punch: SiPunch) -> crate::Result<()> {
        let punch = [Punch {
            raw: &punch.raw,
            ..Default::default()
        }];
        let punches = Punches {
            punches: repeated::Repeated::from_slice(&punch),
            ..Default::default()
        };
        self.send_message::<40>("p", punches, 1).await
    }

    pub async fn setup(&mut self) -> crate::Result<()> {
        let _ = self.bg77.turn_on().await;
        self.config().await?;

        let _ = self.client.mqtt_connect(&mut self.bg77).await;
        Ok(())
    }

    pub async fn mqtt_connect(&mut self) -> crate::Result<()> {
        self.client.mqtt_connect(&mut self.bg77).await
    }

    pub async fn synchronize_time(&mut self) -> Option<chrono::DateTime<chrono::FixedOffset>> {
        self.system_info.current_time(&mut self.bg77, false).await
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
            Either::Second(_) => EVENT_CHANNEL.send(Command::SynchronizeTime(Instant::now())).await,
        }
    }
}

#[embassy_executor::task]
pub async fn send_punch_event_handler(
    send_punch_mutex: &'static SendPunchMutexType,
    si_uart_channel: &'static SiUartChannelType,
) {
    let mut last_reconnect: Option<Instant> = None;
    loop {
        let signal = select3(
            MCH_SIGNAL.wait(),
            EVENT_CHANNEL.receive(),
            si_uart_channel.receive(),
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
                Either3::Second(command) => match command {
                    Command::MqttConnect(force, _) => {
                        if !force
                            && last_reconnect.map(|t| t + Duration::from_secs(60) > Instant::now())
                                == Some(true)
                        {
                            continue;
                        }

                        if let Err(err) = send_punch.mqtt_connect().await {
                            error!("Error connecting to MQTT: {}", err);
                        }
                        last_reconnect = Some(Instant::now());
                    }
                    Command::SynchronizeTime(_) => {
                        let time = send_punch.synchronize_time().await;
                        match time {
                            None => warn!("Cannot get modem time"),
                            Some(time) => {
                                info!("Modem time: {}", format!(30; "{}", time).unwrap().as_str())
                            }
                        }
                    }
                },
                Either3::Third(punch) => match punch {
                    Ok(punch) => {
                        info!(
                            "{} punched {} at {}",
                            punch.card,
                            punch.code,
                            format!(30; "{}", punch.time).unwrap().as_str(),
                        );
                        match send_punch.send_punch(punch).await {
                            Ok(()) => {
                                info!("Sent punch");
                            }
                            Err(err) => {
                                error!("Error while sending punch: {}", err);
                            }
                        }
                    }
                    Err(err) => {
                        error!("Wrong punch: {}", err);
                    }
                },
            }
        }
    }
}
