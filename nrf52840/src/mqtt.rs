use crate::{
    backoff::{BackoffRetries, MQTT_MESSAGES},
    bg77_hw::ModemHw,
    error::Error,
    send_punch::{Command, EVENT_CHANNEL},
};
use core::{marker::PhantomData, str::FromStr};
use defmt::{debug, error, info, warn};
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, WithTimeout};
use heapless::{format, String};
use yaroc_common::{at::response::CommandResponse, RawMutex};

const MQTT_CLIENT_ID: u8 = 0;

static MQTT_EXTRA_TIMEOUT: Duration = Duration::from_millis(300);
pub static ACTIVATION_TIMEOUT: Duration = Duration::from_secs(150);
pub static MQTT_URCS: [Signal<RawMutex, MqttPublishReport>; MQTT_MESSAGES + 1] = [
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
    Signal::new(),
];

pub enum MqttPubStatus {
    Published,
    Retrying(u8),
    Timeout,
    Unknown,
}

pub struct MqttPublishReport {
    pub msg_id: u8,
    pub status: MqttPubStatus,
}

impl MqttPublishReport {
    pub fn new(msg_id: u8, status: u8, retries: Option<&u8>) -> Self {
        let status = match status {
            0 => MqttPubStatus::Published,
            1 => MqttPubStatus::Retrying(*retries.unwrap_or(&0)),
            2 => MqttPubStatus::Timeout,
            _ => MqttPubStatus::Unknown,
        };

        Self { msg_id, status }
    }
}

pub struct MqttConfig {
    pub url: String<40>,
    pub packet_timeout: Duration,
    pub apn: String<30>,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            url: String::from_str("broker.emqx.io").unwrap(),
            packet_timeout: Duration::from_secs(35),
            apn: String::from_str("trail-nbiot.corp").unwrap(),
        }
    }
}

pub struct MqttClient<M: ModemHw> {
    config: MqttConfig,
    msg_id: u8,
    last_successful_send: Instant,
    _phantom: PhantomData<M>,
}

impl<M: ModemHw> MqttClient<M> {
    pub fn new(config: MqttConfig) -> Self {
        Self {
            config,
            msg_id: 0,
            last_successful_send: Instant::now(),
            _phantom: PhantomData,
        }
    }

    async fn network_registration(&mut self, bg77: &mut M) -> crate::Result<()> {
        if self.last_successful_send + ACTIVATION_TIMEOUT * 3 < Instant::now() {
            self.last_successful_send = Instant::now();
            bg77.simple_call_at("E0", None).await?;
            let _ = bg77.call_at("+CGATT=0", ACTIVATION_TIMEOUT).await;
            Timer::after_secs(2).await;
            let _ = bg77.call_at("+CGACT=0,1", ACTIVATION_TIMEOUT).await;
            Timer::after_secs(2).await; // TODO
            bg77.call_at("+CGATT=1", ACTIVATION_TIMEOUT).await?;
        }

        let (_, state) =
            bg77.simple_call_at("+CGACT?", None).await?.parse2::<u8, u8>([0, 1], Some(1))?;
        if state == 0 {
            bg77.call_at("+CGACT=1,1", ACTIVATION_TIMEOUT).await?;
        }

        Ok(())
    }

    pub fn urc_handler(response: &CommandResponse) -> bool {
        match response.command() {
            "QMTSTAT" | "QIURC" => {
                let message = Command::MqttConnect(true, Instant::now());
                if EVENT_CHANNEL.try_send(message).is_err() {
                    error!("Error while sending Mqtt connect command, channel full");
                }
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
                MQTT_URCS[idx].signal(MqttPublishReport::new(values[1], values[2], values.get(3)));
            }
            true
        } else {
            false
        }
    }

    async fn mqtt_open(&self, bg77: &mut M, cid: u8) -> crate::Result<()> {
        let opened = bg77
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
            bg77.simple_call_at(&cmd, Some(ACTIVATION_TIMEOUT)).await?;
        }

        let cmd = format!(50;
            "+QMTCFG=\"timeout\",{cid},{},2,1",
            self.config.packet_timeout.as_secs()
        )?;
        bg77.simple_call_at(&cmd, None).await?;
        let cmd = format!(50;
            "+QMTCFG=\"keepalive\",{cid},{}",
            (self.config.packet_timeout * 3).as_secs()
        )?;
        bg77.simple_call_at(&cmd, None).await?;

        let cmd = format!(100; "+QMTOPEN={cid},\"{}\",1883", self.config.url)?;
        let (_, status) = bg77
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

    pub async fn mqtt_connect(&mut self, bg77: &mut M) -> crate::Result<()> {
        if let Err(err) = self.network_registration(bg77).await {
            error!("Network registration failed: {}", err);
            return Err(err);
        }
        let cid = MQTT_CLIENT_ID;
        self.mqtt_open(bg77, cid).await?;

        let (_, status) = bg77
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
                let (_, res, reason) = bg77
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

    #[allow(dead_code)]
    pub async fn mqtt_disconnect(&mut self, bg77: &mut M, cid: u8) -> Result<(), Error> {
        let cmd = format!(50; "+QMTDISC={cid}")?;
        let (_, result) = bg77
            .simple_call_at(&cmd, Some(self.config.packet_timeout + MQTT_EXTRA_TIMEOUT))
            .await?
            .parse2::<u8, i8>([0, 1], Some(cid))?;
        const MQTT_DISCONNECTED: i8 = 0;
        if result != MQTT_DISCONNECTED {
            return Err(Error::MqttError(result));
        }
        let cmd = format!(50; "+QMTCLOSE={cid}")?;
        let _ = bg77.simple_call_at(&cmd, None).await; // TODO: Why does it fail?
        Ok(())
    }

    pub async fn send_message(
        &mut self,
        bg77: &mut M,
        topic: &str,
        msg: &[u8],
        qos: u8,
    ) -> Result<(), Error> {
        let msg_id = if qos == 0 { 0 } else { self.msg_id + 1 };
        let idx = usize::from(msg_id);
        MQTT_URCS[idx].reset();
        if qos == 1 {
            self.msg_id = (self.msg_id + 1) % u8::try_from(MQTT_MESSAGES).unwrap();
        }

        let cmd = format!(100;
            "+QMTPUB={},{},{},0,\"yar/cee423506cac/{}\",{}", MQTT_CLIENT_ID, msg_id, qos, topic, msg.len(),
        )?;
        bg77.simple_call_at(&cmd, None).await?;
        bg77.call(msg).await?;
        loop {
            let mqtt_report = MQTT_URCS[idx]
                .wait()
                .with_timeout(self.config.packet_timeout + MQTT_EXTRA_TIMEOUT)
                .await
                .map_err(|_| Error::TimeoutError)?;
            // Retries should go into an async loop/queue
            match mqtt_report.status {
                MqttPubStatus::Published => break,
                MqttPubStatus::Retrying(retries) => {
                    warn!("Message ID {} try {} failed", mqtt_report.msg_id, retries);
                }
                MqttPubStatus::Timeout => return Err(Error::TimeoutError),
                MqttPubStatus::Unknown => return Err(Error::ModemError),
            }
        }
        debug!("Message ID {} successfully sent", idx);
        self.last_successful_send = Instant::now();
        Ok(())
    }
}

#[embassy_executor::task]
pub async fn mqtt_backoff_retries() {
    let mut backoff_retries = BackoffRetries::default();
    backoff_retries.r#loop().await;
}
