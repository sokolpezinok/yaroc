use crate::{
    error::Error,
    status::{NrfTemp, Temp},
};
use chrono::{DateTime, FixedOffset, TimeDelta};
use core::str::FromStr;
use defmt::{debug, error, info, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_nrf::{
    gpio::Output,
    peripherals::{P0_17, UARTE1},
    uarte::UarteTx,
};
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Ticker, Timer, WithTimeout};
use femtopb::Message;
use heapless::{format, String};
use yaroc_common::{
    at::{
        response::{AtResponse, CommandResponse},
        uart::{AtUart, RxWithIdle, Tx},
    },
    status::{parse_qlts, MiniCallHome},
};

#[cfg(all(target_abi = "eabihf", target_os = "none"))]
type RawMutex = embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
#[cfg(not(all(target_abi = "eabihf", target_os = "none")))]
type RawMutex = embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

pub type BG77Type = Mutex<RawMutex, Option<BG77<NrfTemp, UarteTx<'static, UARTE1>>>>;

const MQTT_MESSAGES: usize = 5;

static MINIMUM_TIMEOUT: Duration = Duration::from_millis(300);
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

pub struct Config {
    // TODO: this is only MQTT-related, could it be renamed to MqttConfig?
    pub url: String<40>,
    pub pkt_timeout: Duration,
    pub activation_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            url: String::from_str("broker.emqx.io").unwrap(),
            activation_timeout: Duration::from_secs(150),
            pkt_timeout: Duration::from_secs(35),
        }
    }
}

pub struct BG77<S: Temp, T: Tx> {
    uart1: AtUart<T>,
    _modem_pin: Output<'static, P0_17>,
    temp: S,
    boot_time: Option<DateTime<FixedOffset>>,
    config: Config,
    // TODO: refactor out MQTT-related stuff
    client_id: u8,
    msg_id: u8,
    last_successful_send: Instant,
}

impl<S: Temp, T: Tx> BG77<S, T> {
    pub fn new(
        rx1: impl RxWithIdle,
        tx1: T,
        modem_pin: Output<'static, P0_17>,
        temp: S,
        spawner: &Spawner,
        config: Config,
    ) -> Self {
        let uart1 = AtUart::new(rx1, tx1, Self::urc_handler, spawner);
        Self {
            uart1,
            _modem_pin: modem_pin,
            temp,
            client_id: 0,
            msg_id: 0,
            boot_time: None,
            last_successful_send: Instant::now(),
            config,
        }
    }

    fn urc_handler(response: &CommandResponse) -> bool {
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
        self.simple_call("E0", None).await?;
        self.simple_call("+CEREG=2", None).await?;
        self.uart1.call_at("+CGATT=1", self.config.activation_timeout).await?;
        // +QCFG needs +CGATT=1 first
        self.simple_call("+QCFG=\"nwscanseq\",03", None).await?;
        self.simple_call("+QCFG=\"iotopmode\",1,1", None).await?;
        self.simple_call("+QCFG=\"band\",0,0,80000", None).await?;
        Ok(())
    }

    async fn simple_call(
        &mut self,
        cmd: &str,
        response_timeout: Option<Duration>,
    ) -> crate::Result<AtResponse> {
        match response_timeout {
            None => Ok(self.uart1.call_at(cmd, MINIMUM_TIMEOUT).await?),
            Some(response_timeout) => Ok(self
                .uart1
                .call_at_with_response(cmd, MINIMUM_TIMEOUT, response_timeout)
                .await?),
        }
    }

    async fn network_registration(&mut self) -> crate::Result<()> {
        if self.last_successful_send + self.config.activation_timeout * 5 < Instant::now() {
            self.last_successful_send = Instant::now();
            let _ = self.uart1.call_at("+CGATT=0", self.config.activation_timeout).await;
            Timer::after_secs(2).await;
            let _ = self.uart1.call_at("+CGACT=0,1", self.config.activation_timeout).await;
            Timer::after_secs(2).await; // TODO
            self.uart1.call_at("+CGATT=1", self.config.activation_timeout).await?;
        }

        let (_, state) =
            self.simple_call("+CGACT?", None).await?.parse2::<u8, u8>([0, 1], Some(1))?;
        if state == 0 {
            let _ = self.simple_call("+CGDCONT=1,\"IP\",trial-nbiot.corp", None).await;
            self.uart1.call_at("+CGACT=1,1", self.config.activation_timeout).await?;
        }

        Ok(())
    }

    async fn mqtt_open(&mut self, cid: u8) -> crate::Result<()> {
        let opened = self
            .simple_call("+QMTOPEN?", None)
            .await?
            .parse2::<u8, String<40>>([0, 1], Some(cid));
        if let Ok((client_id, url)) = opened.as_ref() {
            if *client_id == cid && *url == self.config.url {
                info!("TCP connection already opened to {}", url.as_str());
                return Ok(());
            }
            // TODO: disconnect an old client
        }

        let cmd = format!(100; "+QMTOPEN={cid},\"{}\",1883", self.config.url)?;
        let (_, status) = self
            .simple_call(&cmd, Some(self.config.activation_timeout))
            .await?
            .parse2::<u8, i8>([0, 1], Some(cid))?;
        if status != 0 {
            error!(
                "Could not open TCP connection to {}",
                self.config.url.as_str()
            );
            return Err(Error::MqttError(status));
        }

        let cmd = format!(50;
            "+QMTCFG=\"timeout\",{cid},{},2,1",
            self.config.pkt_timeout.as_secs()
        )?;
        self.simple_call(&cmd, None).await?;
        let cmd = format!(50;
            "+QMTCFG=\"keepalive\",{cid},{}",
            (self.config.pkt_timeout * 3).as_secs()
        )?;
        self.simple_call(&cmd, None).await?;

        Ok(())
    }

    pub async fn mqtt_connect(&mut self) -> crate::Result<()> {
        if let Err(err) = self.network_registration().await {
            error!("Network registration failed: {}", err);
            return Err(err);
        }
        let cid = self.client_id;
        self.mqtt_open(cid).await?;

        let (_, status) =
            self.simple_call("+QMTCONN?", None).await?.parse2::<u8, u8>([0, 1], Some(cid))?;
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
                    .simple_call(&cmd, Some(self.config.pkt_timeout + MINIMUM_TIMEOUT))
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
            .simple_call(&cmd, Some(self.config.pkt_timeout + MINIMUM_TIMEOUT))
            .await?
            .parse2::<u8, i8>([0, 1], Some(cid))?;
        const MQTT_DISCONNECTED: i8 = 0;
        if result != MQTT_DISCONNECTED {
            return Err(Error::MqttError(result));
        }
        let cmd = format!(50; "+QMTCLOSE={cid}")?;
        let _ = self.simple_call(&cmd, None).await; // TODO: Why does it fail?
        Ok(())
    }

    async fn battery_state(&mut self) -> Result<(u16, u8), Error> {
        let (bcs, volt) = self.simple_call("+CBC", None).await?.parse2::<u8, u16>([1, 2], None)?;
        Ok((volt, bcs))
    }

    async fn signal_info(&mut self) -> Result<(i8, i8, u8, i8), Error> {
        let response = self.simple_call("+QCSQ", None).await?;
        if response.count_response_values() != Ok(5) {
            return Err(Error::NetworkRegistrationError);
        }
        Ok(response.parse4::<i8, i8, u8, i8>([1, 2, 3, 4])?)
    }

    async fn cellid(&mut self) -> Result<u32, Error> {
        self.simple_call("+CEREG?", None)
            .await?
            // TODO: support roaming, that's answer 5
            .parse2::<u32, String<8>>([1, 3], Some(1))
            .map_err(Error::from)
            .and_then(|(_, cell)| u32::from_str_radix(&cell, 16).map_err(|_| Error::ParseError))
    }

    async fn mini_call_home(&mut self) -> MiniCallHome {
        let cpu_temperature = self.temp.cpu_temperature().await;
        let mut mini_call_home = MiniCallHome::default().set_cpu_temperature(cpu_temperature);
        if let Ok((battery_mv, battery_percents)) = self.battery_state().await {
            mini_call_home.set_battery_info(battery_mv, battery_percents);
        }
        if let Ok((rssi_dbm, rsrp_dbm, snr_mult, rsrq_dbm)) = self.signal_info().await {
            mini_call_home.set_signal_info(snr_mult, rssi_dbm, rsrp_dbm, rsrq_dbm);
        }
        if let Ok(cellid) = self.cellid().await {
            mini_call_home.set_cellid(cellid);
        }

        mini_call_home
    }

    async fn send_message<const N: usize>(&mut self, msg: impl Message<'_>) -> Result<(), Error> {
        let mut buf = [0u8; N];
        msg.encode(&mut buf.as_mut_slice()).map_err(|_| Error::BufferTooSmallError)?;
        let len = msg.encoded_len();
        let res = self.send_message_impl(&buf[..len]).await;
        if res.is_err() {
            MQTT_CONNECT_SIGNAL.signal((false, Instant::now()));
        }
        res
    }
    async fn send_message_impl(&mut self, msg: &[u8]) -> Result<(), Error> {
        let cmd = format!(100;
            "+QMTPUB={},{},1,0,\"yar/cee423506cac/status\",{}", self.client_id, self.msg_id + 1, msg.len(),
        )?;
        let idx = usize::from(self.msg_id);
        MQTT_URCS[idx].reset();
        self.simple_call(&cmd, None).await?;
        self.uart1.call(msg, MINIMUM_TIMEOUT).await?;
        self.msg_id = (self.msg_id + 1) % u8::try_from(MQTT_MESSAGES).unwrap();
        loop {
            let (result, retries) = MQTT_URCS[idx]
                .wait()
                .with_timeout(self.config.pkt_timeout + MINIMUM_TIMEOUT)
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

    async fn get_modem_time(&mut self) -> crate::Result<DateTime<FixedOffset>> {
        let modem_clock =
            self.simple_call("+QLTS=2", None).await?.parse1::<String<25>>([0], None)?;
        parse_qlts(&modem_clock).map_err(yaroc_common::error::Error::into)
    }

    pub async fn current_time(&mut self, cached: bool) -> Option<DateTime<FixedOffset>> {
        if self.boot_time.is_none() || !cached {
            let boot_time = self
                .get_modem_time()
                .await
                .map(|time| {
                    let booted = TimeDelta::milliseconds(Instant::now().as_millis() as i64);
                    time.checked_sub_signed(booted).unwrap()
                })
                .ok()?;
            info!("Boot at {}", format!(30; "{}", boot_time).unwrap().as_str());
            self.boot_time = Some(boot_time);
        }
        self.boot_time.map(|boot_time| {
            let delta = TimeDelta::milliseconds(Instant::now().as_millis() as i64);
            boot_time.checked_add_signed(delta).unwrap()
        })
    }

    pub async fn send_mini_call_home(&mut self) -> crate::Result<()> {
        let timestamp = self.current_time(true).await;
        let mini_call_home = self.mini_call_home().await;
        info!("{}", mini_call_home);

        let message = mini_call_home.to_proto(timestamp);
        self.send_message::<250>(message).await
    }

    pub async fn setup(&mut self) -> crate::Result<()> {
        let _ = self.turn_on().await;
        self.config().await?;

        let _ = self.mqtt_connect().await;
        Ok(())
    }

    async fn turn_on(&mut self) -> crate::Result<()> {
        if self.simple_call("", None).await.is_err() {
            self._modem_pin.set_low();
            Timer::after_secs(1).await;
            self._modem_pin.set_high();
            Timer::after_secs(2).await;
            self._modem_pin.set_low();
            let res = self.uart1.read(Duration::from_secs(5)).await?;
            info!("Modem response: {=[?]}", res.as_slice());
            self.uart1.call_at("+CFUN=1,0", Duration::from_secs(15)).await?;
            let res = self.uart1.read(Duration::from_secs(5)).await?;
            info!("Modem response: {=[?]}", res.as_slice());
        }
        Ok(())
    }
}

#[embassy_executor::task]
pub async fn bg77_main_loop(bg77_mutex: &'static BG77Type) {
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
pub async fn bg77_event_handler(bg77_mutex: &'static BG77Type) {
    let mut last_reconnect: Option<Instant> = None;
    loop {
        let signal = select3(
            MCH_SIGNAL.wait(),
            MQTT_CONNECT_SIGNAL.wait(),
            GET_TIME_SIGNAL.wait(),
        )
        .await;
        {
            let mut bg77_unlocked = bg77_mutex.lock().await;
            let bg77 = bg77_unlocked.as_mut().unwrap();
            match signal {
                Either3::First(_) => match bg77.send_mini_call_home().await {
                    Ok(()) => info!("MiniCallHome sent"),
                    Err(err) => error!("Sending of MiniCallHome failed: {}", err),
                },
                Either3::Second((force, _)) => {
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
                Either3::Third(_) => {
                    let time = bg77.current_time(false).await;
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
}
