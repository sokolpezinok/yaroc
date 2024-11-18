use crate::{
    at_utils::{AtUart, URC_CHANNEL},
    error::Error,
};
use chrono::{NaiveDateTime, TimeDelta};
use common::at::split_at_response;
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

pub struct BG77 {
    uart1: AtUart,
    _modem_pin: Output<'static, P0_17>,
    pkt_timeout: Duration,
    activation_timeout: Duration,
    client_id: u8,
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
        let pkt_timeout = Duration::from_secs(30);
        Self {
            uart1,
            _modem_pin: modem_pin,
            activation_timeout,
            pkt_timeout,
            client_id: 0,
        }
    }

    pub async fn urc_handler(&mut self, line: &str) -> crate::Result<()> {
        let (prefix, _rest) = split_at_response(line).ok_or(Error::ParseError)?;
        info!("URC {}", line);
        match prefix {
            "QMTSTAT" | "CEREG" => self.mqtt_connect(self.client_id).await?,
            _ => {
                todo!()
            }
        }
        Ok(())
    }

    pub async fn config(&mut self) -> Result<(), Error> {
        self.uart1.call("E0", MINIMUM_TIMEOUT).await?;
        self.uart1
            .call("+QCFG=\"nwscanseq\",03", MINIMUM_TIMEOUT)
            .await?;
        self.uart1
            .call("+QCFG=\"iotopmode\",1,1", MINIMUM_TIMEOUT)
            .await?;
        self.uart1
            .call("+QCFG=\"band\",0,0,80000", MINIMUM_TIMEOUT)
            .await?;
        self.uart1.call("+CEREG=2", MINIMUM_TIMEOUT).await?;
        Ok(())
    }

    async fn network_registration(&mut self) -> crate::Result<()> {
        self.uart1.call("+CGATT=1", self.activation_timeout).await?;
        let _ = self
            .uart1
            .call("+CGDCONT=1,\"IP\",trial-nbiot.corp", MINIMUM_TIMEOUT)
            .await;
        self.uart1
            .call("+CGACT=1,1", self.activation_timeout)
            .await?;
        //self.uart1
        //    .call("+CGPADDR=1", MINIMUM_TIMEOUT)
        //    .await?
        //    .parse2::<u8, String<50>>([0, 1], None)?;
        Ok(())
    }

    async fn mqtt_open(&mut self, cid: u8) -> crate::Result<()> {
        let opened = self
            .uart1
            .call("+QMTOPEN?", MINIMUM_TIMEOUT)
            .await?
            .parse2::<u8, String<40>>([0, 1], Some(cid));
        if let Ok((client_id, url)) = opened.as_ref() {
            if *client_id == cid && url.as_str() == "broker.emqx.io" {
                info!("TCP connection already opened to {}", url.as_str());
                return Ok(());
            }
            // TODO: disconnect an old client
        }

        let cmd = format!(100; "+QMTOPEN={cid},\"broker.emqx.io\",1883").unwrap();
        let (client_id, status) = self
            .uart1
            .call_with_response(&cmd, MINIMUM_TIMEOUT, self.activation_timeout)
            .await?
            .parse2::<u8, i8>([0, 1], Some(cid))?;
        if status != 0 || client_id != cid {
            return Err(Error::MqttError(status));
        }
        Ok(())
    }

    pub async fn mqtt_connect(&mut self, cid: u8) -> crate::Result<()> {
        self.network_registration().await?;
        self.mqtt_open(cid).await?;

        let cmd = format!(50;
            "+QMTCFG=\"timeout\",{cid},{},2,1",
            self.pkt_timeout.as_secs()
        )
        .unwrap();
        self.uart1.call(&cmd, MINIMUM_TIMEOUT).await?;
        let cmd = format!(50;
            "+QMTCFG=\"keepalive\",{cid},{}",
            (self.pkt_timeout * 3).as_secs()
        )
        .unwrap();
        self.uart1.call(&cmd, MINIMUM_TIMEOUT).await?;

        let connection = self
            .uart1
            .call("+QMTCONN?", MINIMUM_TIMEOUT)
            .await?
            .parse2::<u8, u8>([0, 1], Some(cid));
        const MQTT_INITIALIZING: u8 = 1;
        const MQTT_CONNECTING: u8 = 2;
        const MQTT_CONNECTED: u8 = 3;
        const MQTT_DISCONNECTING: u8 = 4;
        if let Ok((cid, status)) = connection.as_ref() {
            match *status {
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
                    let cmd = format!(50; "+QMTCONN={cid},\"yaroc-nrf52\"").unwrap();
                    let (client_id, res, reason) = self
                        .uart1
                        .call_with_response(&cmd, MINIMUM_TIMEOUT, self.pkt_timeout)
                        .await?
                        .parse3::<u8, u32, i8>([0, 1, 2], Some(cid))?;

                    if client_id == *cid && res == 0 && reason == 0 {
                        Ok(())
                    } else {
                        Err(Error::MqttError(reason))
                    }
                }
                _ => Err(Error::AtError),
            }
        } else {
            Err(Error::AtError)
        }
    }

    pub async fn mqtt_disconnect(&mut self, cid: u8) -> Result<(), Error> {
        let cmd = format!(50; "+QMTDISC={cid}").unwrap();
        let (client_id, result) = self
            .uart1
            .call_with_response(&cmd, MINIMUM_TIMEOUT, self.pkt_timeout)
            .await?
            .parse2::<u8, i8>([0, 1], Some(cid))?;
        const MQTT_DISCONNECTED: i8 = 0;
        if !(client_id == cid && result == MQTT_DISCONNECTED) {
            return Err(Error::MqttError(result));
        }
        let cmd = format!(50; "+QMTCLOSE={cid}").unwrap();
        let _ = self.uart1.call(&cmd, MINIMUM_TIMEOUT).await; // TODO: this fails
        Ok(())
    }

    async fn battery_mv(&mut self) -> Result<u32, Error> {
        let (_, bcs, volt) = self
            .uart1
            .call("+CBC", MINIMUM_TIMEOUT)
            .await?
            .parse3::<i32, i32, u32>([0, 1, 2], None)?;
        info!("Batt: {}mV, {}%", volt, bcs);
        Ok(volt)
    }

    async fn signal_info(&mut self) -> Result<SignalInfo, Error> {
        let (mut rssi_dbm, rsrp_dbm, snr_mult, rsrq_dbm) = self
            .uart1
            .call("+QCSQ", MINIMUM_TIMEOUT)
            .await?
            // TODO: Handle +QCSQ: "NOSERVICE"
            .parse4::<i8, i8, u8, i8>([1, 2, 3, 4])?;
        let snr_db = f64::from(snr_mult) / 5. - 20.;
        if rssi_dbm == 0 {
            rssi_dbm = rsrp_dbm - rsrq_dbm;
        }

        let cellid = self
            .uart1
            .call("+CEREG?", MINIMUM_TIMEOUT)
            .await?
            // TODO: can be +CEREG: 2,2
            .parse3::<u32, u32, String<8>>([0, 1, 3], None)
            .map_err(Error::from)
            .and_then(|(_, _, cell)| {
                u32::from_str_radix(cell.as_str(), 16).map_err(|_| Error::ParseError)
            })
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
            "+QMTPUBEX={},0,0,0,\"topic/pub\",\"{text}\"", self.client_id
        )
        .unwrap();
        let response = self
            .uart1
            .call_with_response(&cmd, MINIMUM_TIMEOUT, self.pkt_timeout * 2)
            .await;
        if response.is_err() {
            let _ = self.uart1.call("+QMTCONN?", MINIMUM_TIMEOUT).await;
            response?;
        }
        Ok(())
    }

    async fn get_time(&mut self) -> crate::Result<NaiveDateTime> {
        let modem_clock = self
            .uart1
            .call("+CCLK?", MINIMUM_TIMEOUT)
            .await?
            .parse1::<String<20>>([0], None)?;

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
        let _ = self.turn_on().await;
        unwrap!(self.config().await);
        let _ = self.mqtt_connect(self.client_id).await;
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

    #[allow(dead_code)]
    async fn turn_on(&mut self) -> crate::Result<()> {
        if let Err(_) = self.uart1.call("", MINIMUM_TIMEOUT).await {
            self._modem_pin.set_low();
            Timer::after_millis(1000).await;
            self._modem_pin.set_high();
            Timer::after_millis(2000).await;
            self._modem_pin.set_low();
            let res = self.uart1.read(Duration::from_secs(5)).await?;
            info!("Modem response: {=[?]}", res.as_slice());
            self.uart1
                .call("+CFUN=1,0", Duration::from_secs(15))
                .await?;
        }
        Ok(())
    }
}

#[embassy_executor::task]
pub async fn bg77_main_loop(bg77_mutex: &'static BG77Type) {
    {
        let mut bg77_unlocked = bg77_mutex.lock().await;
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
                    let res = bg77.urc_handler(line.as_str()).await;
                    if let Err(err) = res {
                        error!("Error while processing URC: {}", err);
                    }
                }
            }
        }
    }
}
