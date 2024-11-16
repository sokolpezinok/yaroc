use crate::{
    at_utils::{AtUart, URC_CHANNEL},
    error::Error,
};
use chrono::{NaiveDateTime, TimeDelta};
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
const CLIENT_ID: u8 = 1;

pub struct BG77 {
    uart1: AtUart,
    _modem_pin: Output<'static, P0_17>,
    pkt_timeout: Duration,
    activation_timeout: Duration,
}

fn urc_handler(prefix: &str, rest: &str) -> bool {
    match prefix {
        "QMTSTAT" => true,
        // The CEREG URC is shorter, normal one has 5 values
        "CEREG" => rest.split(',').count() == 4,
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
        let activation_timeout = Duration::from_secs(140);
        let pkt_timeout = Duration::from_secs(18); // 30
        Self {
            uart1,
            _modem_pin: modem_pin,
            activation_timeout,
            pkt_timeout,
        }
    }

    pub async fn urc_handler(&mut self, line: &str) {
        info!("Callback: {}", line);
    }

    pub async fn config(&mut self) -> Result<(), Error> {
        self.uart1.call("ATE0", MINIMUM_TIMEOUT).await?;
        self.uart1
            .call("AT+QCFG=\"nwscanseq\",03", MINIMUM_TIMEOUT)
            .await?;
        self.uart1
            .call("AT+QCFG=\"iotopmode\",1,1", MINIMUM_TIMEOUT)
            .await?;
        self.uart1
            .call("AT+QCFG=\"band\",0,0,80000", MINIMUM_TIMEOUT)
            .await?;
        self.uart1
            .call("AT+CGATT=1", self.activation_timeout)
            .await?;
        self.uart1.call("AT+CEREG=2", MINIMUM_TIMEOUT).await?;
        let _ = self
            .uart1
            .call("AT+CGDCONT=1,\"IP\",trial-nbiot.corp", MINIMUM_TIMEOUT)
            .await;
        self.uart1
            .call("AT+CGPADDR=1", MINIMUM_TIMEOUT)
            .await?
            .parse2::<u8, String<50>>([0, 1])?;
        Ok(())
    }

    async fn mqtt_open(&mut self) -> crate::Result<()> {
        let opened = self
            .uart1
            .call("AT+QMTOPEN?", MINIMUM_TIMEOUT)
            .await?
            .parse2::<u8, String<40>>([0, 1]);
        if let Ok((CLIENT_ID, url)) = opened.as_ref() {
            if url.as_str() == "broker.emqx.io" {
                info!("TCP connection already opened to {}", url.as_str());
                return Ok(());
            }
            // TODO: disconnect an old client
        }

        let at_command = format!(100; "AT+QMTOPEN={CLIENT_ID},\"broker.emqx.io\",1883").unwrap();
        let (client_id, status) = self
            .uart1
            .call_with_response(&at_command, MINIMUM_TIMEOUT, self.activation_timeout)
            .await?
            .parse2::<u8, i8>([0, 1])?;
        if status != 0 || client_id != CLIENT_ID {
            return Err(Error::MqttError(status));
        }
        Ok(())
    }

    pub async fn mqtt_connect(&mut self) -> Result<(), Error> {
        self.mqtt_open().await?;

        let command = format!(50;
            "AT+QMTCFG=\"timeout\",{CLIENT_ID},{},2,1",
            self.pkt_timeout.as_secs()
        )
        .unwrap();
        self.uart1.call(&command, MINIMUM_TIMEOUT).await?;
        let command = format!(50;
            "AT+QMTCFG=\"keepalive\",{CLIENT_ID},{}",
            (self.pkt_timeout * 3).as_secs()
        )
        .unwrap();
        self.uart1.call(&command, MINIMUM_TIMEOUT).await?;

        let connection = self
            .uart1
            .call("AT+QMTCONN?", MINIMUM_TIMEOUT)
            .await?
            .parse2::<u8, u8>([0, 1]);
        const MQTT_INITIALIZING: u8 = 1;
        const MQTT_CONNECTING: u8 = 2;
        const MQTT_CONNECTED: u8 = 3;
        const MQTT_DISCONNECTING: u8 = 4;
        if let Ok((CLIENT_ID, status)) = connection.as_ref() {
            match *status {
                MQTT_CONNECTED => {
                    info!("Already connected to MQTT");
                }
                _ => {
                    info!("Connecting or being disconnected from MQTT"); // TODO
                }
            }
            return Ok(());
        }
        info!("Connecting to MQTT");
        let command = format!(50; "AT+QMTCONN={CLIENT_ID},\"yaroc-nrf52\"").unwrap();
        let (client_id, res, reason) = self
            .uart1
            .call_with_response(&command, MINIMUM_TIMEOUT, self.pkt_timeout)
            .await?
            .parse3::<u8, u32, i8>([0, 1, 2])?;

        if client_id == CLIENT_ID && res == 0 && reason == 0 {
            Ok(())
        } else {
            Err(Error::MqttError(reason))
        }
    }

    pub async fn mqtt_disconnect(&mut self) -> Result<(), Error> {
        let command = format!(50; "AT+QMTDISC={CLIENT_ID}").unwrap();
        let (client_id, result) = self
            .uart1
            .call_with_response(&command, MINIMUM_TIMEOUT, self.pkt_timeout)
            .await?
            .parse2::<u8, i8>([0, 1])?;
        const MQTT_DISCONNECTED: i8 = 0;
        if !(client_id == CLIENT_ID && result == MQTT_DISCONNECTED) {
            return Err(Error::MqttError(result));
        }
        let command = format!(50; "AT+QMTCLOSE={CLIENT_ID}").unwrap();
        let _ = self.uart1.call(&command, MINIMUM_TIMEOUT).await; // TODO: this fails
        Ok(())
    }

    async fn battery_mv(&mut self) -> Result<u32, Error> {
        let (_, bcs, volt) = self
            .uart1
            .call("AT+CBC", MINIMUM_TIMEOUT)
            .await?
            .parse3::<i32, i32, u32>([0, 1, 2])?;
        info!("Batt: {}mV, {}%", volt, bcs);
        Ok(volt)
    }

    async fn signal_info(&mut self) -> Result<SignalInfo, Error> {
        let (mut rssi_dbm, rsrp_dbm, snr_mult, rsrq_dbm) = self
            .uart1
            .call("AT+QCSQ", MINIMUM_TIMEOUT)
            .await?
            // TODO: Handle +QCSQ: "NOSERVICE"
            .parse4::<i8, i8, u8, i8>([1, 2, 3, 4])?;
        let snr_db = f64::from(snr_mult) / 5. - 20.;
        if rssi_dbm == 0 {
            rssi_dbm = rsrp_dbm - rsrq_dbm;
        }

        let cellid = self
            .uart1
            .call("AT+CEREG?", MINIMUM_TIMEOUT)
            .await?
            // TODO: can be +CEREG: 2,2
            .parse3::<u32, u32, String<8>>([0, 1, 3])
            .and_then(|(_, _, cell)| {
                u32::from_str_radix(cell.as_str(), 16).map_err(|_| Error::ParseError)
            })
            .ok();
        self.uart1
            .call("AT+QMTCONN?", MINIMUM_TIMEOUT)
            .await?
            .parse2::<u8, u8>([0, 1])?;
        let signal_info = SignalInfo {
            rssi_dbm: if rssi_dbm == 0 { None } else { Some(rssi_dbm) },
            snr_db: Some(snr_db as f32),
            cellid,
        };
        Ok(signal_info)
    }

    async fn send_text(&mut self, text: &str) -> Result<(), Error> {
        let command = format!(100;
            "AT+QMTPUBEX={CLIENT_ID},0,0,0,\"topic/pub\",\"{text}\""
        )
        .unwrap();
        let _response = self
            .uart1
            .call_with_response(&command, MINIMUM_TIMEOUT, self.pkt_timeout * 2)
            .await?;
        Ok(())
    }

    async fn get_time(&mut self) -> crate::Result<NaiveDateTime> {
        let modem_clock = self
            .uart1
            .call("AT+CCLK?", MINIMUM_TIMEOUT)
            .await?
            .parse1::<String<20>>([0])?;

        let datetime =
            NaiveDateTime::parse_from_str(modem_clock.as_str(), "%y/%m/%d,%H:%M:%S+04").unwrap();

        Ok(datetime)
    }

    pub async fn send_signal_info(&mut self) {
        let signal_info = self.signal_info().await;
        info!("Signal info: {}", signal_info);

        if let Ok(signal_info) = signal_info {
            let bat_mv = self.battery_mv().await.unwrap_or_default();
            let text = format!(50; "{}mV;{}dB;{:X}", bat_mv, signal_info.snr_db.unwrap_or_default(), signal_info.cellid.unwrap_or_default()).unwrap();
            let _ = self.send_text(text.as_str()).await;
        }
    }

    pub async fn setup(&mut self) {
        //self._turn_on().await;
        unwrap!(self.config().await);
        let _ = self.mqtt_connect().await;
        let now_ms = Instant::now().as_millis();
        let boot_time = self.get_time().await.map(|time| {
            time.checked_sub_signed(TimeDelta::milliseconds(now_ms as i64))
                .unwrap()
        });
        info!(
            "Boot at {}",
            format!(30; "{}", boot_time.unwrap()).unwrap().as_str()
        );
    }

    async fn _turn_on(&mut self) {
        self._modem_pin.set_low();
        Timer::after_millis(1000).await;
        self._modem_pin.set_high();
        Timer::after_millis(2000).await;
        self._modem_pin.set_low();
        // TODO: read the response
    }
}

#[embassy_executor::task]
pub async fn bg77_main_loop(bg77_mutex: &'static BG77Type) {
    {
        let mut bg77_unlocked = bg77_mutex.lock().await;
        info!("Unlocked");
        if let Some(bg77) = bg77_unlocked.as_mut() {
            bg77.setup().await;
        }
    }

    let mut ticker = Ticker::every(Duration::from_secs(10));
    loop {
        ticker.next().await;
        let mut bg77_unlocked = bg77_mutex.lock().await;
        if let Some(bg77) = bg77_unlocked.as_mut() {
            bg77.send_signal_info().await;
        }
    }
    //unwrap!(self.mqtt_disconnect().await);
}

#[embassy_executor::task]
pub async fn bg77_urc_handler(bg77_mutex: &'static BG77Type) {
    loop {
        let urc = URC_CHANNEL.receive().await;
        match urc {
            Err(err) => {
                error!("Error received over URC channel: {}", err);
            }
            Ok(line) => {
                let mut bg77_unlocked = bg77_mutex.lock().await;
                if let Some(bg77) = bg77_unlocked.as_mut() {
                    bg77.urc_handler(line.as_str()).await;
                }
            }
        }
    }
}
