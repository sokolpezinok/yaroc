use crate::{
    at_utils::{AtUart, URC_CHANNEL},
    error::Error,
};
use chrono::{DateTime, FixedOffset, TimeDelta};
use common::{
    at::{split_at_response, AtResponse},
    status::{parse_qlts, MiniCallHome},
};
use core::str::FromStr;
use defmt::{debug, error, info, unwrap};
use embassy_executor::Spawner;
use embassy_nrf::{
    gpio::Output,
    peripherals::{P0_17, TIMER0, UARTE1},
    temp::Temp,
    uarte::{UarteRxWithIdle, UarteTx},
};
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, ThreadModeRawMutex};
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Instant, Ticker, Timer};
use femtopb::Message as _;
use heapless::{format, String, Vec};

pub type BG77Type = Mutex<ThreadModeRawMutex, Option<BG77>>;

const QMTPUB_VALUES: usize = 4;
const MQTT_MESSAGES: usize = 5;

static MINIMUM_TIMEOUT: Duration = Duration::from_millis(300);
static MQTT_URCS: [Signal<CriticalSectionRawMutex, u8>; MQTT_MESSAGES] = [
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
];

pub struct Config {
    url: String<40>,
    pkt_timeout: Duration,
    activation_timeout: Duration,
}

pub struct BG77 {
    uart1: AtUart,
    _modem_pin: Output<'static, P0_17>,
    temp: Temp<'static>,
    client_id: u8,
    msg_id: u8,
    boot_time: Option<DateTime<FixedOffset>>,
    config: Config,
    last_successful_send: Instant,
    last_reattach: Instant,
    last_reconnect: Instant,
}

fn urc_classifier(prefix: &str, rest: &str) -> bool {
    match prefix {
        "QMTSTAT" | "QIURC" => true,
        "CEREG" => {
            // The CEREG URC is shorter, normal one has 5 values
            let value_count = rest.split(',').count();
            value_count == 1 || value_count == 4
        }
        "QMTPUB" => {
            let res: Result<Vec<u8, QMTPUB_VALUES>, _> = rest
                .split(',')
                .map(|val| str::parse(val).map_err(|_| Error::ParseError))
                .collect();
            if let Ok(values) = res {
                values[1] != 0
            } else {
                false
            }
        }
        _ => false,
    }
}

impl BG77 {
    pub fn new(
        rx1: UarteRxWithIdle<'static, UARTE1, TIMER0>,
        tx1: UarteTx<'static, UARTE1>,
        modem_pin: Output<'static, P0_17>,
        temp: Temp<'static>,
        spawner: &Spawner,
    ) -> Self {
        let uart1 = AtUart::new(rx1, tx1, urc_classifier, spawner);
        let activation_timeout = Duration::from_secs(150);
        let pkt_timeout = Duration::from_secs(30);
        Self {
            uart1,
            _modem_pin: modem_pin,
            temp,
            client_id: 0,
            msg_id: 0,
            boot_time: None,
            last_successful_send: Instant::now(),
            last_reattach: Instant::now(),
            last_reconnect: Instant::now(),
            config: Config {
                url: String::from_str("broker.emqx.io").unwrap(),
                pkt_timeout,
                activation_timeout,
            },
        }
    }

    pub async fn urc_handler(line: &str, bg77_mutex: &'static BG77Type) -> crate::Result<()> {
        let (prefix, rest) = split_at_response(line).ok_or(Error::ParseError)?;
        match prefix {
            "QMTSTAT" | "CEREG" => {
                let mut bg77_unlocked = bg77_mutex.lock().await;
                let bg77 = bg77_unlocked.as_mut().unwrap();
                bg77.mqtt_connect().await?;
            }
            "QMTPUB" => {
                let res: Result<Vec<u8, QMTPUB_VALUES>, _> = rest
                    .split(',')
                    .map(|val| str::parse(val).map_err(|_| Error::ParseError))
                    .collect();
                if let Ok(values) = res {
                    // TODO: get client ID
                    if values[1] > 0 && values[0] == 0 {
                        let msg_id = values[1];
                        info!("Response to message ID {}", msg_id);
                        let idx = usize::from(msg_id) - 1;
                        if idx < MQTT_URCS.len() {
                            MQTT_URCS[idx].signal(values[2]);
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub async fn config(&mut self) -> Result<(), Error> {
        self.simple_call("E0").await?;
        self.simple_call("+CEREG=2").await?;
        self.uart1.call_at("+CGATT=1", self.config.activation_timeout).await?;
        // +QCFG needs +CGATT=1 first
        self.simple_call("+QCFG=\"nwscanseq\",03").await?;
        self.simple_call("+QCFG=\"iotopmode\",1,1").await?;
        self.simple_call("+QCFG=\"band\",0,0,80000").await?;
        Ok(())
    }

    async fn simple_call(&mut self, cmd: &str) -> crate::Result<AtResponse> {
        self.uart1.call_at(cmd, MINIMUM_TIMEOUT).await
    }

    async fn call_with_response(
        &mut self,
        cmd: &str,
        response_timeout: Duration,
    ) -> crate::Result<AtResponse> {
        self.uart1.call_at_with_response(cmd, MINIMUM_TIMEOUT, response_timeout).await
    }

    async fn network_registration(&mut self) -> crate::Result<()> {
        if self.last_successful_send + self.config.activation_timeout < Instant::now()
            && self.last_reattach + self.config.activation_timeout < Instant::now()
        {
            let _ = self.uart1.call_at("+CGATT=0", self.config.activation_timeout).await;
            Timer::after_secs(2).await;
            let _ = self.uart1.call_at("+CGACT=0,1", self.config.activation_timeout).await;
            Timer::after_secs(2).await; // TODO
            self.uart1.call_at("+CGATT=1", self.config.activation_timeout).await?;
            self.last_reattach = Instant::now();
        }

        let (_, state) = self.simple_call("+CGACT?").await?.parse2::<u8, u8>([0, 1], Some(1))?;
        if state == 0 {
            let _ = self.simple_call("+CGDCONT=1,\"IP\",trial-nbiot.corp").await;
            self.uart1.call_at("+CGACT=1,1", self.config.activation_timeout).await?;
        }

        Ok(())
    }

    async fn mqtt_open(&mut self, cid: u8) -> crate::Result<()> {
        let opened =
            self.simple_call("+QMTOPEN?").await?.parse2::<u8, String<40>>([0, 1], Some(cid));
        if let Ok((client_id, url)) = opened.as_ref() {
            if *client_id == cid && *url == self.config.url {
                info!("TCP connection already opened to {}", url.as_str());
                return Ok(());
            }
            // TODO: disconnect an old client
        }

        let cmd = format!(100; "+QMTOPEN={cid},\"{}\",1883", self.config.url).unwrap();
        let (_, status) = self
            .call_with_response(&cmd, self.config.activation_timeout)
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
        )
        .unwrap();
        self.simple_call(&cmd).await?;
        let cmd = format!(50;
            "+QMTCFG=\"keepalive\",{cid},{}",
            (self.config.pkt_timeout * 3).as_secs()
        )
        .unwrap();
        self.simple_call(&cmd).await?;

        Ok(())
    }

    pub async fn mqtt_connect(&mut self) -> crate::Result<()> {
        self.network_registration().await?;
        if self.last_reconnect + self.config.pkt_timeout > Instant::now() {
            return Ok(());
        }
        let cid = self.client_id;
        self.mqtt_open(cid).await?;

        let (_, status) =
            self.simple_call("+QMTCONN?").await?.parse2::<u8, u8>([0, 1], Some(cid))?;
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
                let cmd = format!(50; "+QMTCONN={cid},\"nrf52840\"").unwrap();
                let (_, res, reason) = self
                    .call_with_response(&cmd, self.config.pkt_timeout)
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
        let cmd = format!(50; "+QMTDISC={cid}").unwrap();
        let (_, result) = self
            .call_with_response(&cmd, self.config.pkt_timeout)
            .await?
            .parse2::<u8, i8>([0, 1], Some(cid))?;
        const MQTT_DISCONNECTED: i8 = 0;
        if result != MQTT_DISCONNECTED {
            return Err(Error::MqttError(result));
        }
        let cmd = format!(50; "+QMTCLOSE={cid}").unwrap();
        let _ = self.simple_call(&cmd).await; // TODO: Why does it fail?
        Ok(())
    }

    async fn battery_state(&mut self) -> Result<(u16, u8), Error> {
        let (bcs, volt) = self.simple_call("+CBC").await?.parse2::<u8, u16>([1, 2], None)?;
        Ok((volt, bcs))
    }

    async fn signal_info(&mut self) -> Result<(i8, i8, u8, i8), Error> {
        let response = self.simple_call("+QCSQ").await?;
        if response.count_response_values() != Ok(5) {
            return Err(Error::NetworkRegistrationError);
        }
        Ok(response.parse4::<i8, i8, u8, i8>([1, 2, 3, 4])?)
    }

    async fn cellid(&mut self) -> Result<u32, Error> {
        self.simple_call("+CEREG?")
            .await?
            // TODO: support roaming, that's answer 5
            .parse2::<u32, String<8>>([1, 3], Some(1))
            .map_err(Error::from)
            .and_then(|(_, cell)| u32::from_str_radix(&cell, 16).map_err(|_| Error::ParseError))
    }

    async fn mini_call_home(&mut self) -> MiniCallHome {
        let temp = self.temp.read().await;
        let cpu_temperature = temp.to_num::<i8>(); // TODO: Has two fractional bits
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

    async fn send_message(&mut self, msg: &[u8]) -> Result<(), Error> {
        let res = self.send_message_impl(msg).await;
        if res.is_err() {
            let _ = self.mqtt_connect().await;
        }
        res
    }
    async fn send_message_impl(&mut self, msg: &[u8]) -> Result<(), Error> {
        let cmd = format!(100;
            "+QMTPUB={},{},1,0,\"yar/b827eab91544/status\",{}", self.client_id, self.msg_id + 1, msg.len(),
        )
        .unwrap();
        self.simple_call(&cmd).await?;
        self.uart1.call(msg, MINIMUM_TIMEOUT).await?;
        let idx = usize::from(self.msg_id);
        self.msg_id = (self.msg_id + 1) % u8::try_from(MQTT_MESSAGES).unwrap();
        let result = with_timeout(self.config.pkt_timeout * 2, MQTT_URCS[idx].wait())
            .await
            .map_err(|_| Error::TimeoutError)?;
        if result != 0 {
            return Err(Error::MqttError(result as i8));
        }
        self.last_successful_send = Instant::now();
        Ok(())
    }

    async fn get_modem_time(&mut self) -> crate::Result<DateTime<FixedOffset>> {
        let modem_clock = self.simple_call("+QLTS=2").await?.parse1::<String<25>>([0], None)?;
        parse_qlts(&modem_clock).map_err(common::error::Error::into)
    }

    async fn current_time(&mut self) -> Option<DateTime<FixedOffset>> {
        if self.boot_time.is_none() {
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
        let timestamp = self.current_time().await;
        let mini_call_home = self.mini_call_home().await;
        info!("{}", mini_call_home);

        let message = mini_call_home.to_proto(timestamp);
        let mut buf = [0u8; 200];
        message
            .encode(&mut buf.as_mut_slice())
            .map_err(|_| Error::BufferTooSmallError)?;
        let len = message.encoded_len();
        self.send_message(&buf[..len]).await
    }

    pub async fn setup(&mut self) -> crate::Result<()> {
        let _ = self.turn_on().await;
        unwrap!(self.config().await);

        let _ = self.mqtt_connect().await;
        Ok(())
    }

    #[allow(dead_code)]
    async fn turn_on(&mut self) -> crate::Result<()> {
        if self.simple_call("").await.is_err() {
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

    let mut ticker = Ticker::every(Duration::from_secs(20));
    loop {
        let mut bg77_unlocked = bg77_mutex.lock().await;
        let bg77 = bg77_unlocked.as_mut().unwrap();
        if let Err(err) = bg77.send_mini_call_home().await {
            error!("Sending of Mini Call Home failed: {}", err);
        }
        ticker.next().await;
    }
}

#[embassy_executor::task]
pub async fn bg77_urc_handler(bg77_mutex: &'static BG77Type) {
    loop {
        let urc = URC_CHANNEL.receive().await;
        match urc {
            Ok(line) => {
                debug!("Got URC: {}", line.as_str());
                let res = BG77::urc_handler(line.as_str(), bg77_mutex).await;
                if let Err(err) = res {
                    error!("Error while processing URC: {}", err);
                }
            }
            Err(err) => {
                error!("Error received over URC channel: {}", err);
            }
        }
    }
}
