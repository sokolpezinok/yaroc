use crate::{
    bg77_hw::{ModemHw, ModemPin},
    error::Error,
    si_uart::SiUartChannelType,
    status::{NrfTemp, Temp},
};
use chrono::{DateTime, FixedOffset};
use core::str::FromStr;
use defmt::{debug, error, info, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{select, select4, Either, Either4};
use embassy_nrf::{gpio::Output, peripherals::UARTE1, uarte::UarteTx};
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Ticker, Timer, WithTimeout};
use femtopb::Message;
use heapless::{format, String};
use yaroc_common::{
    at::{
        response::CommandResponse,
        uart::{AtUart, RxWithIdle, Tx},
    },
    punch::SiPunch,
    RawMutex,
};

pub type BG77Type = BG77<NrfTemp, UarteTx<'static, UARTE1>, Output<'static>>;
pub type BG77MutexType = Mutex<RawMutex, Option<BG77Type>>;

const MQTT_MESSAGES: usize = 5;
const MQTT_CLIENT_ID: u8 = 0;

static MQTT_EXTRA_TIMEOUT: Duration = Duration::from_millis(500);
static ACTIVATION_TIMEOUT: Duration = Duration::from_secs(150);
static MQTT_URCS: [Signal<RawMutex, (u8, u8)>; MQTT_MESSAGES] = [
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
];
// MiniCallHome signal
static MCH_SIGNAL: Signal<RawMutex, Instant> = Signal::new();
static GET_TIME_SIGNAL: Signal<RawMutex, Instant> = Signal::new();
static MQTT_CONNECT_SIGNAL: Signal<RawMutex, (bool, Instant)> = Signal::new();

pub struct MqttConfig {
    pub url: String<40>,
    pub packet_timeout: Duration,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            url: String::from_str("broker.emqx.io").unwrap(),
            packet_timeout: Duration::from_secs(35),
        }
    }
}

pub struct BG77<S: Temp, T: Tx, P: ModemPin> {
    pub uart1: AtUart<T>,
    pub modem_pin: P,
    pub temp: S,
    pub boot_time: Option<DateTime<FixedOffset>>,
    config: MqttConfig,
    msg_id: u8,
    last_successful_send: Instant,
}

impl<S: Temp, T: Tx, P: ModemPin> BG77<S, T, P> {
    pub fn new(
        rx1: impl RxWithIdle,
        tx1: T,
        modem_pin: P,
        temp: S,
        spawner: &Spawner,
        config: MqttConfig,
    ) -> Self {
        let uart1 = AtUart::new(rx1, tx1, Self::urc_handler, spawner);
        Self {
            uart1,
            modem_pin,
            temp,
            msg_id: 0,
            boot_time: None,
            last_successful_send: Instant::now(),
            config,
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
        if values[1] > 0 && values[0] == 0 {
            let msg_id = values[1];
            let idx = usize::from(msg_id) - 1;
            if idx < MQTT_URCS.len() {
                MQTT_URCS[idx].signal((values[2], *values.get(3).unwrap_or(&0)));
            }
            true
        } else {
            false
        }
    }

    pub async fn config(&mut self) -> Result<(), Error> {
        self.simple_call_at("E0", None).await?;
        self.simple_call_at("+CEREG=2", None).await?;
        self.call_at("+CGATT=1", ACTIVATION_TIMEOUT).await?;
        // +QCFG needs +CGATT=1 first
        self.simple_call_at("+QCFG=\"nwscanseq\",03", None).await?;
        self.simple_call_at("+QCFG=\"iotopmode\",1,1", None).await?;
        self.simple_call_at("+QCFG=\"band\",0,0,80000", None).await?;
        Ok(())
    }

    async fn network_registration(&mut self) -> crate::Result<()> {
        if self.last_successful_send + ACTIVATION_TIMEOUT * 3 < Instant::now() {
            self.last_successful_send = Instant::now();
            let _ = self.call_at("+CGATT=0", ACTIVATION_TIMEOUT).await;
            Timer::after_secs(2).await;
            let _ = self.call_at("+CGACT=0,1", ACTIVATION_TIMEOUT).await;
            Timer::after_secs(2).await; // TODO
            self.call_at("+CGATT=1", ACTIVATION_TIMEOUT).await?;
        }

        let (_, state) =
            self.simple_call_at("+CGACT?", None).await?.parse2::<u8, u8>([0, 1], Some(1))?;
        if state == 0 {
            let _ = self.simple_call_at("+CGDCONT=1,\"IP\",trial-nbiot.corp", None).await;
            self.call_at("+CGACT=1,1", ACTIVATION_TIMEOUT).await?;
        }

        Ok(())
    }

    async fn mqtt_open(&mut self, cid: u8) -> crate::Result<()> {
        let opened = self
            .simple_call_at("+QMTOPEN?", None)
            .await?
            .parse2::<u8, String<40>>([0, 1], Some(cid));
        if let Ok((MQTT_CLIENT_ID, url)) = opened {
            if *url == self.config.url {
                info!("TCP connection already opened to {}", url.as_str());
                return Ok(());
            }
            warn!(
                "Connected to the wrong broker {}, will disconnect",
                url.as_str()
            );
            let cmd = format!(50; "+QMTCLOSE={cid}")?;
            self.simple_call_at(&cmd, Some(ACTIVATION_TIMEOUT)).await?;
        }

        let cmd = format!(50;
            "+QMTCFG=\"timeout\",{cid},{},2,1",
            self.config.packet_timeout.as_secs()
        )?;
        self.simple_call_at(&cmd, None).await?;
        let cmd = format!(50;
            "+QMTCFG=\"keepalive\",{cid},{}",
            (self.config.packet_timeout * 3).as_secs()
        )?;
        self.simple_call_at(&cmd, None).await?;

        let cmd = format!(100; "+QMTOPEN={cid},\"{}\",1883", self.config.url)?;
        let (_, status) = self
            .simple_call_at(&cmd, Some(ACTIVATION_TIMEOUT))
            .await?
            .parse2::<u8, i8>([0, 1], Some(cid))?;
        if status != 0 {
            error!(
                "Could not open TCP connection to {}",
                self.config.url.as_str()
            );
            return Err(Error::MqttError(status));
        }

        Ok(())
    }

    pub async fn mqtt_connect(&mut self) -> crate::Result<()> {
        if let Err(err) = self.network_registration().await {
            error!("Network registration failed: {}", err);
            return Err(err);
        }
        let cid = MQTT_CLIENT_ID;
        self.mqtt_open(cid).await?;

        let (_, status) = self
            .simple_call_at("+QMTCONN?", None)
            .await?
            .parse2::<u8, u8>([0, 1], Some(cid))?;
        const MQTT_INITIALIZING: u8 = 1;
        const MQTT_CONNECTING: u8 = 2;
        const MQTT_CONNECTED: u8 = 3;
        const MQTT_DISCONNECTING: u8 = 4;
        match status {
            MQTT_CONNECTED => {
                info!("Already connected to MQTT");
                Ok(())
            }
            MQTT_DISCONNECTING | MQTT_CONNECTING => {
                info!("Connecting or disconnecting from MQTT");
                Ok(())
            }
            MQTT_INITIALIZING => {
                info!("Will connect to MQTT");
                let cmd = format!(50; "+QMTCONN={cid},\"nrf52840\"")?;
                let (_, res, reason) = self
                    .simple_call_at(&cmd, Some(self.config.packet_timeout + MQTT_EXTRA_TIMEOUT))
                    .await?
                    .parse3::<u8, u32, i8>([0, 1, 2], Some(cid))?;

                if res == 0 && reason == 0 {
                    Ok(())
                } else {
                    Err(Error::MqttError(reason))
                }
            }
            _ => Err(Error::ModemError),
        }
    }

    pub async fn mqtt_disconnect(&mut self, cid: u8) -> Result<(), Error> {
        let cmd = format!(50; "+QMTDISC={cid}")?;
        let (_, result) = self
            .simple_call_at(&cmd, Some(self.config.packet_timeout + MQTT_EXTRA_TIMEOUT))
            .await?
            .parse2::<u8, i8>([0, 1], Some(cid))?;
        const MQTT_DISCONNECTED: i8 = 0;
        if result != MQTT_DISCONNECTED {
            return Err(Error::MqttError(result));
        }
        let cmd = format!(50; "+QMTCLOSE={cid}")?;
        let _ = self.simple_call_at(&cmd, None).await; // TODO: Why does it fail?
        Ok(())
    }

    async fn send_message<const N: usize>(&mut self, msg: impl Message<'_>) -> Result<(), Error> {
        let mut buf = [0u8; N];
        msg.encode(&mut buf.as_mut_slice()).map_err(|_| Error::BufferTooSmallError)?;
        let len = msg.encoded_len();
        let res = self.send_message_impl(&buf[..len], 0).await;
        if res.is_err() {
            MQTT_CONNECT_SIGNAL.signal((false, Instant::now()));
        }
        res
    }

    async fn send_message_impl(&mut self, msg: &[u8], qos: u8) -> Result<(), Error> {
        let msg_id = if qos == 0 { 0 } else { self.msg_id + 1 };
        if qos != 1 {
            self.msg_id = (self.msg_id + 1) % u8::try_from(MQTT_MESSAGES).unwrap();
        }

        let cmd = format!(100;
            "+QMTPUB={},{},{},0,\"yar/cee423506cac/status\",{}", MQTT_CLIENT_ID, msg_id, qos, msg.len(),
        )?;
        let idx = usize::from(self.msg_id);
        MQTT_URCS[idx].reset();
        self.simple_call_at(&cmd, None).await?;
        self.call(msg).await?;
        loop {
            let (result, retries) = MQTT_URCS[idx]
                .wait()
                .with_timeout(self.config.packet_timeout + MQTT_EXTRA_TIMEOUT)
                .await
                .map_err(|_| Error::TimeoutError)?;
            match result {
                0 => break,
                1 => {
                    warn!("Message ID {} try {} failed", idx + 1, retries);
                }
                2 => {
                    return Err(Error::TimeoutError);
                }
                _ => {
                    return Err(Error::ModemError);
                }
            }
        }
        debug!("Message ID {} successfully sent", idx);
        self.last_successful_send = Instant::now();
        Ok(())
    }

    pub async fn send_mini_call_home(&mut self) -> crate::Result<()> {
        let mini_call_home = self.mini_call_home().await.ok_or(Error::ModemError)?;
        self.send_message::<250>(mini_call_home.to_proto()).await
    }

    pub async fn send_punch(&mut self, punch: SiPunch) -> crate::Result<()> {
        self.send_message_impl(&punch.raw, 1).await
    }

    pub async fn setup(&mut self) -> crate::Result<()> {
        let _ = self.turn_on().await;
        self.config().await?;

        let _ = self.mqtt_connect().await;
        Ok(())
    }
}

#[embassy_executor::task]
pub async fn bg77_main_loop(bg77_mutex: &'static BG77MutexType) {
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
pub async fn bg77_event_handler(
    bg77_mutex: &'static BG77MutexType,
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
            let mut bg77_unlocked = bg77_mutex.lock().await;
            let bg77 = bg77_unlocked.as_mut().unwrap();
            match signal {
                Either4::First(_) => match bg77.send_mini_call_home().await {
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

                    if let Err(err) = bg77.mqtt_connect().await {
                        error!("Error connecting to MQTT: {}", err);
                    }
                    last_reconnect = Some(Instant::now());
                }
                Either4::Third(_) => {
                    let time = bg77.current_time(false).await;
                    match time {
                        None => warn!("Cannot get modem time"),
                        Some(time) => {
                            info!("Modem time: {}", format!(30; "{}", time).unwrap().as_str())
                        }
                    }
                }
                Either4::Fourth(punch) => match punch {
                    Ok(punch) => match bg77.send_punch(punch).await {
                        Ok(()) => {
                            info!("Sent punch");
                        }
                        Err(err) => {
                            error!("Error while sending punch: {}", err);
                        }
                    },
                    Err(err) => {
                        error!("Wrong punch: {}", err);
                    }
                },
            }
        }
    }
}
