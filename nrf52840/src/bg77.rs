use crate::{at_utils::AtUart, error::Error};
use defmt::{info, unwrap};
use embassy_executor::Spawner;
use embassy_nrf::{
    gpio::Output,
    peripherals::{P0_17, TIMER0, UARTE1},
    uarte::{UarteRxWithIdle, UarteTx},
};
use embassy_time::{Duration, Timer};
use heapless::{format, String};

static MINIMUM_TIMEOUT: Duration = Duration::from_millis(300);
const CLIENT_ID: u32 = 0;

pub struct BG77 {
    uart1: AtUart,
    _modem_pin: Output<'static, P0_17>,
    pkt_timeout: Duration,
    activation_timeout: Duration,
}

fn callback_dispatcher(prefix: &str, _rest: &str) -> bool {
    // TODO: Improve this
    match prefix {
        "QMTSTAT" => true,
        "QMTPUB" => true,
        _ => false,
    }
}

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
        let uart1 = AtUart::new(rx1, tx1, callback_dispatcher, spawner);
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

    async fn signal_info(&mut self) -> Result<SignalInfo, Error> {
        let (rssi_dbm, snr_mult) = self
            .uart1
            .call("AT+QCSQ", MINIMUM_TIMEOUT, &[1, 3])
            .await?
            .parse2::<i32, i32>()?;
        let snr = f64::from(snr_mult - 100) / 5.0;
        let (_, status, cellid) = self
            .uart1
            .call("AT+CEREG?", MINIMUM_TIMEOUT, &[0, 1, 3])
            .await?
            .parse3::<u32, u32, String<8>>()?;
        let cellid = u32::from_str_radix(cellid.as_str(), 16).ok();
        self.uart1
            .call("AT+QMTCONN?", MINIMUM_TIMEOUT, &[0, 1])
            .await?;
        let _ = self.uart1.read(self.pkt_timeout).await;
        let signal_info = SignalInfo {
            rssi_dbm: if rssi_dbm == 0 { None } else { Some(rssi_dbm) },
            snr: Some(snr as f32),
            cellid,
        };
        info!(
            "Signal info: {:?} {:?} {:?}",
            signal_info.rssi_dbm, signal_info.snr, signal_info.cellid
        );
        Ok(signal_info)
    }

    pub async fn experiment(&mut self) {
        //self._turn_on().await;
        unwrap!(self.config().await);
        let _ = self.mqtt_connect().await;

        let command = format!(100;
            "AT+QMTPUBEX={CLIENT_ID},0,0,0,\"topic/pub\",\"Hello from embassy\""
        )
        .unwrap();
        let _ = self.uart1.call(&command, self.pkt_timeout * 2, &[]).await;
        for _ in 0..5 {
            unwrap!(self.signal_info().await);
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
