use crate::{at_utils::AtUart, error::Error};
use defmt::{info, unwrap};
use embassy_executor::Spawner;
use embassy_nrf::{
    gpio::Output,
    peripherals::{P0_17, TIMER0, UARTE1},
    uarte::{UarteRxWithIdle, UarteTx},
};
use embassy_time::{Duration, Ticker, Timer};
use heapless::{format, String};

static MINIMUM_TIMEOUT: Duration = Duration::from_millis(300);
const CLIENT_ID: u32 = 0;

pub struct BG77 {
    uart1: AtUart,
    _modem_pin: Output<'static, P0_17>,
    pkt_timeout: Duration,
    activation_timeout: Duration,
}

fn urc_handler(prefix: &str, rest: &str) -> bool {
    match prefix {
        "QMTSTAT" => true,
        "CEREG" => rest.split(',').count() == 4, // The URC is shorter, normal one has 5 values
        "QMTPUB" => true,
        _ => false,
    }
}

#[derive(defmt::Format)]
struct SignalInfo {
    pub rssi_dbm: Option<i32>,
    pub snr: Option<f32>,
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
        let activation_timeout = Duration::from_secs(40); // 140
        let pkt_timeout = Duration::from_secs(12); // 30
        Self {
            uart1,
            _modem_pin: modem_pin,
            activation_timeout,
            pkt_timeout,
        }
    }

    pub async fn config(&mut self) -> Result<(), Error> {
        self.uart1.call("ATE0", MINIMUM_TIMEOUT, &[]).await?;
        self.uart1
            .call("AT+CGATT=1", self.activation_timeout, &[])
            .await?;
        self.uart1.call("AT+CEREG=2", MINIMUM_TIMEOUT, &[]).await?;
        let _ = self
            .uart1
            .call("AT+CGDCONT=1,\"IP\",trial-nbiot.corp", MINIMUM_TIMEOUT, &[])
            .await;
        self.uart1
            .call("AT+CGPADDR=1", MINIMUM_TIMEOUT, &[0, 1])
            .await?;
        Ok(())
    }

    pub async fn mqtt_connect(&mut self) -> Result<(), Error> {
        let at_command = format!(100; "AT+QMTOPEN={CLIENT_ID},\"broker.emqx.io\",1883").unwrap();
        let res = self
            .uart1
            .call_with_response(
                &at_command,
                MINIMUM_TIMEOUT,
                self.activation_timeout,
                &[0, 1],
            )
            .await?
            .parse2::<u32, u32>()?;
        info!("MQTT open: {} {}", res.0, res.1);

        self.uart1
            .call("AT+QMTOPEN?", MINIMUM_TIMEOUT, &[0, 1])
            .await?
            .parse2::<u32, String<20>>()?;
        // Good response: +QMTOPEN: <client_id>,"broker.emqx.io",1883

        let command = format!(50;
            "AT+QMTCFG=\"timeout\",{CLIENT_ID},{},2,1",
            self.pkt_timeout.as_secs()
        )
        .unwrap();
        self.uart1.call(&command, MINIMUM_TIMEOUT, &[]).await?;
        let command = format!(50;
            "AT+QMTCFG=\"keepalive\",{CLIENT_ID},{}",
            (self.pkt_timeout * 3).as_secs()
        )
        .unwrap();
        self.uart1.call(&command, MINIMUM_TIMEOUT, &[]).await?;
        let command = format!(50; "AT+QMTCONN={CLIENT_ID},\"client-embassy\"").unwrap();
        let (client_id, res, reason) = self
            .uart1
            .call_with_response(&command, MINIMUM_TIMEOUT, self.pkt_timeout, &[0, 1, 2])
            .await?
            .parse3::<u32, u32, u32>()?;
        info!("MQTT connection: {} {} {}", client_id, res, reason);

        self.uart1
            .call("AT+QMTCONN?", MINIMUM_TIMEOUT, &[0, 1])
            .await?;
        // Good response +QMTCONN: <client_id>,3
        Ok(())
    }

    pub async fn mqtt_disconnect(&mut self) -> Result<(), Error> {
        let command = format!(50; "AT+QMTDISC={CLIENT_ID}").unwrap();
        let (client_id, result) = self
            .uart1
            .call_with_response(&command, MINIMUM_TIMEOUT, self.pkt_timeout, &[0, 1])
            .await?
            .parse2::<u32, u32>()?;
        info!("MQTT disconnect: {} {}", client_id, result);
        let command = format!(50; "AT+QMTCLOSE={CLIENT_ID}").unwrap();
        self.uart1.call(&command, MINIMUM_TIMEOUT, &[]).await?;
        Ok(())
    }

    async fn battery_mv(&mut self) -> Result<u32, Error> {
        let (_, bcs, volt) = self
            .uart1
            .call("AT+CBC", MINIMUM_TIMEOUT, &[0, 1, 2])
            .await?
            .parse3::<i32, i32, u32>()?;
        info!("Batt: {}mV, {}%", volt, bcs);
        Ok(volt)
    }

    async fn signal_info(&mut self) -> Result<SignalInfo, Error> {
        let (rssi_dbm, snr_mult) = self
            .uart1
            .call("AT+QCSQ", MINIMUM_TIMEOUT, &[1, 3])
            .await?
            .parse2::<i32, i32>()?;
        let snr = f64::from(snr_mult - 100) / 5.0;

        let cellid = match self
            .uart1
            .call("AT+CEREG?", MINIMUM_TIMEOUT, &[0, 1, 3])
            .await?
            .parse3::<u32, u32, String<8>>()
        {
            Err(_) => None,
            Ok((_, _, cell)) => u32::from_str_radix(cell.as_str(), 16).ok(),
        };
        self.uart1
            .call("AT+QMTCONN?", MINIMUM_TIMEOUT, &[0, 1])
            .await?;
        let signal_info = SignalInfo {
            rssi_dbm: if rssi_dbm == 0 { None } else { Some(rssi_dbm) },
            snr: Some(snr as f32),
            cellid,
        };
        Ok(signal_info)
    }

    async fn send_text(&mut self, text: &str) -> Result<(), Error> {
        let command = format!(100;
            "AT+QMTPUBEX={CLIENT_ID},0,0,0,\"topic/pub\",\"{text}\""
        )
        .unwrap();
        let _response = self.uart1.call(&command, self.pkt_timeout * 2, &[]).await?;
        Ok(())
    }

    pub async fn experiment(&mut self) {
        //self._turn_on().await;
        unwrap!(self.config().await);
        let _ = self.mqtt_connect().await;

        let mut ticker = Ticker::every(Duration::from_secs(10));
        for _ in 0..5 {
            let signal_info = self.signal_info().await;
            info!("Signal info: {}", signal_info);

            if let Ok(signal_info) = signal_info {
                let bat_mv = self.battery_mv().await.unwrap_or_default();
                let text = format!(50; "{}mV;{}dB;{:X}", bat_mv, signal_info.snr.unwrap_or_default(), signal_info.cellid.unwrap_or_default()).unwrap();
                let _ = self.send_text(text.as_str()).await;
            }
            ticker.next().await;
        }
        unwrap!(self.mqtt_disconnect().await);
    }

    async fn _turn_on(&mut self) {
        self._modem_pin.set_low();
        Timer::after_millis(1000).await;
        self._modem_pin.set_high();
        Timer::after_millis(2000).await;
        self._modem_pin.set_low();
        Timer::after_millis(100).await;
    }
}
