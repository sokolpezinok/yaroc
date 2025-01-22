use crate::{
    bg77_hw::{Bg77, ModemHw},
    error::Error,
    mqtt::{MqttClient, MqttConfig, ACTIVATION_TIMEOUT, MQTT_URCS},
    si_uart::SiUartChannelType,
    system_info::{NrfTemp, SystemInfo, Temp},
};
use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{select, select4, Either, Either4};
use embassy_nrf::{
    gpio::Output,
    peripherals::{TIMER0, UARTE1},
    uarte::{UarteRxWithIdle, UarteTx},
};
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Ticker};
use femtopb::{repeated, Message};
use heapless::format;
use yaroc_common::{
    at::response::CommandResponse,
    proto::{Punch, Punches},
    punch::SiPunch,
    RawMutex,
};

pub type SendPunchType = SendPunch<
    NrfTemp,
    Bg77<UarteTx<'static, UARTE1>, UarteRxWithIdle<'static, UARTE1, TIMER0>, Output<'static>>,
>;
pub type SendPunchMutexType = Mutex<RawMutex, Option<SendPunchType>>;

// MiniCallHome signal
static MCH_SIGNAL: Signal<RawMutex, Instant> = Signal::new();
static GET_TIME_SIGNAL: Signal<RawMutex, Instant> = Signal::new();
static MQTT_CONNECT_SIGNAL: Signal<RawMutex, (bool, Instant)> = Signal::new();

pub struct SendPunch<T: Temp, M: ModemHw> {
    pub bg77: M,
    client: MqttClient,
    system_info: SystemInfo<T>,
}

impl<T: Temp, M: ModemHw> SendPunch<T, M> {
    pub fn new(mut bg77: M, temp: T, spawner: &Spawner, config: MqttConfig) -> Self {
        bg77.spawn(Self::urc_handler, spawner);
        Self {
            bg77,
            client: MqttClient::new(config),
            system_info: SystemInfo::<T>::new(temp),
        }
    }

    pub fn urc_handler(response: &CommandResponse) -> bool {
        match response.command() {
            "QMTSTAT" | "QIURC" => {
                MQTT_CONNECT_SIGNAL.signal((true, Instant::now()));
                true
            }
            "QMTPUB" => Self::qmtpub_handler(response),
            _ => false,
        }
    }

    fn qmtpub_handler(response: &CommandResponse) -> bool {
        let values = match response.parse_values::<u8>() {
            Ok(values) => values,
            Err(_) => {
                return false;
            }
        };

        // TODO: get client ID
        if values[0] == 0 {
            let idx = usize::from(values[1]);
            if idx < MQTT_URCS.len() {
                MQTT_URCS[idx].signal((values[2], *values.get(3).unwrap_or(&0)));
            }
            true
        } else {
            false
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
            MQTT_CONNECT_SIGNAL.signal((false, Instant::now()));
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
pub async fn send_punch_main_loop(bg77_mutex: &'static SendPunchMutexType) {
    {
        let mut bg77_unlocked = bg77_mutex.lock().await;
        let bg77 = bg77_unlocked.as_mut().unwrap();
        if let Err(err) = bg77.setup().await {
            error!("Setup failed: {}", err);
        }
    }

    let mut mch_ticker = Ticker::every(Duration::from_secs(20));
    let mut get_time_ticker = Ticker::every(Duration::from_secs(300));
    loop {
        match select(mch_ticker.next(), get_time_ticker.next()).await {
            Either::First(_) => MCH_SIGNAL.signal(Instant::now()),
            Either::Second(_) => GET_TIME_SIGNAL.signal(Instant::now()),
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
        let signal = select4(
            MCH_SIGNAL.wait(),
            MQTT_CONNECT_SIGNAL.wait(),
            GET_TIME_SIGNAL.wait(),
            si_uart_channel.receive(),
        )
        .await;
        {
            let mut send_punch_unlocked = send_punch_mutex.lock().await;
            let send_punch = send_punch_unlocked.as_mut().unwrap();
            match signal {
                Either4::First(_) => match send_punch.send_mini_call_home().await {
                    Ok(()) => info!("MiniCallHome sent"),
                    Err(err) => error!("Sending of MiniCallHome failed: {}", err),
                },
                Either4::Second((force, _)) => {
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
                Either4::Third(_) => {
                    let time = send_punch.synchronize_time().await;
                    match time {
                        None => warn!("Cannot get modem time"),
                        Some(time) => {
                            info!("Modem time: {}", format!(30; "{}", time).unwrap().as_str())
                        }
                    }
                }
                Either4::Fourth(punch) => match punch {
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
