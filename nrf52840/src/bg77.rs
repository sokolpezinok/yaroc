use crate::{
    at_utils::{AtUart, URC_CHANNEL},
    error::Error,
};
use chrono::{NaiveDateTime, TimeDelta};
use common::at::{split_at_response, AtResponse};
use core::str::FromStr;
use defmt::{error, info, unwrap};
use embassy_executor::Spawner;
use embassy_nrf::{
    gpio::Output,
    peripherals::{P0_17, TIMER0, UARTE1},
    uarte::{UarteRxWithIdle, UarteTx},
};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant, Ticker, Timer};
use heapless::{format, String};

pub type BG77Type = Mutex<ThreadModeRawMutex, Option<BG77>>;

static MINIMUM_TIMEOUT: Duration = Duration::from_millis(300);

pub struct Config {
    url: String<40>,
    pkt_timeout: Duration,
    activation_timeout: Duration,
}

pub struct BG77 {
    uart1: AtUart,
    _modem_pin: Output<'static, P0_17>,
    client_id: u8,
    boot_time: Option<NaiveDateTime>,
    config: Config,
    last_successful_send: Instant,
}

fn urc_handler(prefix: &str, rest: &str) -> bool {
    match prefix {
        "QMTSTAT" => true,
        "QIURC" => true,
        "CEREG" => {
            // The CEREG URC is shorter, normal one has 5 values
            let value_count = rest.split(',').count();
            value_count == 1 || value_count == 4
        }
        _ => false,
    }
}

#[derive(defmt::Format)]
struct SignalInfo {
    pub rssi_dbm: Option<i8>,
    pub snr_db: Option<f32>,
    pub cellid: Option<u32>,
}

impl BG77 {
    pub fn new(
        rx1: UarteRxWithIdle<'static, UARTE1, TIMER0>,
        tx1: UarteTx<'static, UARTE1>,
        modem_pin: Output<'static, P0_17>,
        spawner: &Spawner,
    ) -> Self {
        let uart1 = AtUart::new(rx1, tx1, urc_handler, spawner);
        let activation_timeout = Duration::from_secs(150);
        let pkt_timeout = Duration::from_secs(30);
        Self {
            uart1,
            _modem_pin: modem_pin,
            client_id: 0,
            boot_time: None,
            last_successful_send: Instant::now(),
            config: Config {
                url: String::from_str("broker.emqx.io").unwrap(),
                pkt_timeout,
                activation_timeout,
            },
        }
    }

    pub async fn urc_handler(&mut self, line: &str) -> crate::Result<()> {
        let (prefix, _rest) = split_at_response(line).ok_or(Error::ParseError)?;
        info!("URC {}", line);
        match prefix {
            "QMTSTAT" | "CEREG" => self.mqtt_connect(self.client_id).await?,
            "QUIRC" => {}
            _ => {}
        }
        Ok(())
    }

    pub async fn config(&mut self) -> Result<(), Error> {
        self.simple_call("E0").await?;
        Ok(())
    }

    async fn simple_call(&mut self, cmd: &str) -> crate::Result<AtResponse> {
        self.uart1.call(cmd, MINIMUM_TIMEOUT).await
    }

    async fn call_with_response(
        &mut self,
        cmd: &str,
        response_timeout: Duration,
    ) -> crate::Result<AtResponse> {
        self.uart1.call_with_response(cmd, MINIMUM_TIMEOUT, response_timeout).await
    }

    async fn network_registration(&mut self) -> crate::Result<()> {
        if self.last_successful_send + Duration::from_secs(40) < Instant::now() {
            let _ = self.uart1.call("+CGATT=0", self.config.activation_timeout).await;
            let _ = self.uart1.call("+CGACT=0,1", self.config.activation_timeout).await;
            Timer::after_secs(10).await; // TODO
        }
        self.uart1.call("+CGATT=1", self.config.activation_timeout).await?;

        let (_, state) = self.simple_call("+CGACT?").await?.parse2::<u8, u8>([0, 1], Some(1))?;
        if state == 0 {
            self.simple_call("+CGDCONT=1,\"IP\",trial-nbiot.corp").await?;
            self.uart1.call("+CGACT=1,1", self.config.activation_timeout).await?;
        }
        self.simple_call("+QCFG=\"nwscanseq\",03").await?;
        self.simple_call("+QCFG=\"band\",0,0,80000").await?;
        self.simple_call("+CEREG=2").await?;

        // TODO: find out why we sometimes don't get accurate time reading
        let now_ms = Instant::now().as_millis();
        let boot_time = self
            .get_time()
            .await
            .map(|time| time.checked_sub_signed(TimeDelta::milliseconds(now_ms as i64)).unwrap())?;
        info!("Boot at {}", format!(30; "{}", boot_time).unwrap().as_str());
        self.boot_time = Some(boot_time);
        Ok(())
    }

    async fn mqtt_open(&mut self, cid: u8) -> crate::Result<()> {
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
            return Err(Error::MqttError(status));
        }
        Ok(())
    }

    pub async fn mqtt_connect(&mut self, cid: u8) -> crate::Result<()> {
        self.network_registration().await?;
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
            _ => Err(Error::AtError),
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

    async fn battery_mv(&mut self) -> Result<u32, Error> {
        let (_, bcs, volt) =
            self.simple_call("+CBC").await?.parse3::<i32, i32, u32>([0, 1, 2], None)?;
        info!("Batt: {}mV, {}%", volt, bcs);
        Ok(volt)
    }

    async fn signal_info(&mut self) -> Result<SignalInfo, Error> {
        let response = self.simple_call("+QCSQ").await?;
        if response.count_response_values() != Ok(5) {
            return Err(Error::NetworkRegistrationError);
        }
        let (mut rssi_dbm, rsrp_dbm, snr_mult, rsrq_dbm) =
            response.parse4::<i8, i8, u8, i8>([1, 2, 3, 4])?;
        let snr_db = f64::from(snr_mult) / 5. - 20.;
        if rssi_dbm == 0 {
            rssi_dbm = rsrp_dbm - rsrq_dbm;
        }

        let cellid = self
            .simple_call("+CEREG?")
            .await?
            // TODO: support roaming, that's answer 5
            .parse2::<u32, String<8>>([1, 3], Some(1))
            .map_err(Error::from)
            .and_then(|(_, cell)| u32::from_str_radix(&cell, 16).map_err(|_| Error::ParseError))
            .ok();
        let signal_info = SignalInfo {
            rssi_dbm: if rssi_dbm == 0 { None } else { Some(rssi_dbm) },
            snr_db: Some(snr_db as f32),
            cellid,
        };
        Ok(signal_info)
    }

    async fn send_text(&mut self, text: &str) -> Result<(), Error> {
        let cmd = format!(100;
            "+QMTPUBEX={},0,0,0,\"yar\",\"{text}\"", self.client_id
        )
        .unwrap();
        let response = self.call_with_response(&cmd, self.config.pkt_timeout * 2).await;
        if response.is_err() {
            self.mqtt_connect(self.client_id).await?;
            response?;
        }
        self.last_successful_send = Instant::now();
        Ok(())
    }

    async fn get_time(&mut self) -> crate::Result<NaiveDateTime> {
        let modem_clock = self.simple_call("+CCLK?").await?.parse1::<String<20>>([0], None)?;
        NaiveDateTime::parse_from_str(&modem_clock, "%y/%m/%d,%H:%M:%S+04")
            .map_err(|_| Error::ParseError)
    }

    pub async fn send_signal_info(&mut self) {
        let signal_info = self.signal_info().await;
        info!("Signal info: {}", signal_info);

        if let Ok(signal_info) = signal_info {
            let bat_mv = self.battery_mv().await.unwrap_or_default();

            let time_str = match self.boot_time {
                None => String::new(),
                Some(boot_time) => {
                    let delta = TimeDelta::milliseconds(Instant::now().as_millis() as i64);
                    format!(30; "{}", boot_time.checked_add_signed(delta).unwrap()).unwrap()
                }
            };
            let text = format!(50; "{};{}mV;{}dB;{:X}", &time_str, bat_mv, signal_info.snr_db.unwrap_or_default(), signal_info.cellid.unwrap_or_default()).unwrap();
            let _ = self.send_text(&text).await;
        }
    }

    pub async fn setup(&mut self) -> crate::Result<()> {
        let _ = self.turn_on().await;
        unwrap!(self.config().await);

        let _ = self.mqtt_connect(self.client_id).await;
        Ok(())
    }

    #[allow(dead_code)]
    async fn turn_on(&mut self) -> crate::Result<()> {
        if self.simple_call("").await.is_err() {
            self._modem_pin.set_low();
            Timer::after_millis(1000).await;
            self._modem_pin.set_high();
            Timer::after_millis(2000).await;
            self._modem_pin.set_low();
            let res = self.uart1.read(Duration::from_secs(5)).await?;
            info!("Modem response: {=[?]}", res.as_slice());
            self.uart1.call("+CFUN=1,0", Duration::from_secs(15)).await?;
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

    let mut ticker = Ticker::every(Duration::from_secs(10));
    loop {
        ticker.next().await;
        let mut bg77_unlocked = bg77_mutex.lock().await;
        let bg77 = bg77_unlocked.as_mut().unwrap();
        bg77.send_signal_info().await;
    }
    //unwrap!(self.mqtt_disconnect().await);
}

#[embassy_executor::task]
pub async fn bg77_urc_handler(bg77_mutex: &'static BG77Type) {
    loop {
        let urc = URC_CHANNEL.receive().await;
        match urc {
            Ok(line) => {
                let mut bg77_unlocked = bg77_mutex.lock().await;
                let bg77 = bg77_unlocked.as_mut().unwrap();
                let res = bg77.urc_handler(&line).await;
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
