use crate::{
    bg77_hw::ModemHw,
    error::Error,
    send_punch::{Command, SendPunchMutexType, EVENT_CHANNEL},
};
use core::{marker::PhantomData, str::FromStr};
use defmt::{debug, error, info, warn};
use embassy_executor::Spawner;
use embassy_nrf::{peripherals::RNG, rng::Rng};
use embassy_time::{Duration, Instant, Timer, WithTimeout};
use heapless::{format, String};
use yaroc_common::{
    at::{
        mqtt::{MqttStatus, StatusCode},
        response::CommandResponse,
    },
    backoff::{
        BackoffCommand, BackoffRetries, PunchMsg, Random, SendPunchFn, CMD_FOR_BACKOFF,
        PUNCH_QUEUE_SIZE,
    },
    punch::RawPunch,
};

const MQTT_CLIENT_ID: u8 = 0;

static MQTT_EXTRA_TIMEOUT: Duration = Duration::from_millis(300);
pub static ACTIVATION_TIMEOUT: Duration = Duration::from_secs(150);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MqttQos {
    Q0 = 0,
    Q1 = 1,
    // 2 is unsupported
}

#[derive(Clone)]
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

#[embassy_executor::task]
pub async fn backoff_retries_loop(mut backoff_retries: BackoffRetries<Bg77SendPunchFn, NrfRandom>) {
    backoff_retries.r#loop().await;
}

#[derive(Clone, Copy)]
pub struct Bg77SendPunchFn {
    send_punch_mutex: &'static SendPunchMutexType,
    packet_timeout: Duration,
}

impl Bg77SendPunchFn {
    pub fn new(send_punch_mutex: &'static SendPunchMutexType, packet_timeout: Duration) -> Self {
        Self {
            send_punch_mutex,
            packet_timeout,
        }
    }
}

#[embassy_executor::task(pool_size = PUNCH_QUEUE_SIZE)]
async fn bg77_send_punch_fn(
    msg: PunchMsg,
    jitter_after_connect: Duration,
    send_punch_fn: Bg77SendPunchFn,
) {
    BackoffRetries::<Bg77SendPunchFn, NrfRandom>::try_sending_with_retries(
        msg,
        jitter_after_connect,
        send_punch_fn,
    )
    .await
}

impl SendPunchFn for Bg77SendPunchFn {
    async fn send_punch(&mut self, punch: &PunchMsg) -> crate::Result<()> {
        let mut send_punch = self
            .send_punch_mutex
            .lock()
            // TODO: We avoid deadlock by adding a timeout, there might be better solutions
            .with_timeout(self.packet_timeout)
            .await
            .map_err(|_| Error::TimeoutError)?;
        send_punch.as_mut().unwrap().send_punch_impl(punch.punch, punch.msg_id).await
    }

    fn spawn(self, msg: PunchMsg, jitter_after_connect: Duration, spawner: Spawner) {
        spawner.must_spawn(bg77_send_punch_fn(msg, jitter_after_connect, self));
    }
}

pub struct MqttClient<M: ModemHw> {
    config: MqttConfig,
    last_successful_send: Instant,
    punch_cnt: u16,
    _phantom: PhantomData<M>,
}

// Find a better location for it
struct NrfRandom {
    rng: Rng<'static, RNG>,
}

impl Random for NrfRandom {
    async fn u16(&mut self) -> u16 {
        let mut bytes = [0, 0];
        self.rng.fill_bytes(&mut bytes).await;
        u16::from_be_bytes(bytes)
    }
}

impl<M: ModemHw> MqttClient<M> {
    pub fn new(
        send_punch_mutex: &'static SendPunchMutexType,
        config: MqttConfig,
        rng: Rng<'static, RNG>,
        spawner: Spawner,
    ) -> Self {
        let send_punch_for_backoff = Bg77SendPunchFn::new(send_punch_mutex, config.packet_timeout);
        let rng = NrfRandom { rng };
        let backoff_retries =
            BackoffRetries::new(send_punch_for_backoff, rng, Duration::from_secs(10), 23);
        spawner.must_spawn(backoff_retries_loop(backoff_retries));

        Self {
            config,
            last_successful_send: Instant::now(),
            punch_cnt: 0,
            _phantom: PhantomData,
        }
    }

    async fn network_registration(&mut self, bg77: &mut M) -> crate::Result<()> {
        if self.last_successful_send + self.config.packet_timeout * 4 < Instant::now() {
            self.last_successful_send = Instant::now();
            bg77.simple_call_at("E0", None).await?;
            let _ = bg77.call_at("+CGATT=0", ACTIVATION_TIMEOUT).await;
            Timer::after_secs(2).await;
            let _ = bg77.call_at("+CGACT=0,1", ACTIVATION_TIMEOUT).await;
            bg77.call_at("+CGATT=1", ACTIVATION_TIMEOUT).await?;
            Timer::after_secs(2).await;
            return Ok(());
        }

        let state = bg77.simple_call_at("+CGATT?", None).await?.parse1::<u8>([0], None)?;
        if state == 0 {
            bg77.call_at("+CGATT=1", ACTIVATION_TIMEOUT).await?;
            // CGATT=1 needs additional time and reading from modem
            Timer::after_secs(1).await;
            let response = bg77.read().await;
            if let Ok(response) = response {
                if !response.lines().is_empty() {
                    debug!("Read {=[?]} after CGATT=1", response.lines());
                }
            }
        }
        // TODO: should we do something with the result?
        let (_, _) =
            bg77.simple_call_at("+CGACT?", None).await?.parse2::<u8, u8>([0, 1], Some(1))?;

        info!("Already registered to network");
        Ok(())
    }

    pub fn urc_handler(response: &CommandResponse) -> bool {
        match response.command() {
            "QMTSTAT" | "QIURC" => {
                let message = Command::MqttConnect(true, Instant::now());
                if response.command() == "QMTSTAT" {
                    if CMD_FOR_BACKOFF.try_send(BackoffCommand::MqttDisconnected).is_err() {
                        error!("Error while sending MQTT disconnect notification, channel full");
                    }
                }
                if EVENT_CHANNEL.try_send(message).is_err() {
                    error!("Error while sending MQTT connect command, channel full");
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
            let status = MqttStatus::from_bg77_qmtpub(values[1] as u16, values[2], values.get(3));
            if status.msg_id > 0 {
                // This should cause an update of self.last_successful_send (if published)
                if CMD_FOR_BACKOFF.try_send(BackoffCommand::Status(status)).is_err() {
                    error!("Error while sending MQTT message notification, channel full");
                }
                true
            } else {
                false
            }
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
            (self.config.packet_timeout * 2).as_secs()
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
                    info!("Successfully connected to MQTT");
                    if CMD_FOR_BACKOFF.try_send(BackoffCommand::MqttConnected).is_err() {
                        error!("Error while sending MQTT connect notification, channel full");
                    }
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
        qos: MqttQos,
        msg_id: u16,
    ) -> Result<(), Error> {
        let cmd = format!(100;
            "+QMTPUB={},{},{},0,\"yar/cee423506cac/{}\",{}", MQTT_CLIENT_ID, msg_id, qos as u8, topic, msg.len(),
        )?;
        bg77.simple_call_at(&cmd, None).await?;

        let response = bg77.call(msg, "+QMTPUB", qos == MqttQos::Q0).await?;
        if qos == MqttQos::Q0 {
            let (msg_id, status) = response.parse2::<u16, u8>([1, 2], None)?;
            let status = MqttStatus::from_bg77_qmtpub(msg_id, status, None);
            if status.code == StatusCode::Published {
                self.last_successful_send = Instant::now();
            }
        }
        Ok(())
    }

    /// Schedules punch and returns its Punch ID
    pub async fn schedule_punch(&mut self, punch: RawPunch) -> u16 {
        // TODO: what if channel is full?
        let punch_id = self.punch_cnt;
        CMD_FOR_BACKOFF.send(BackoffCommand::PublishPunch(punch, punch_id)).await;
        self.punch_cnt += 1;
        punch_id
    }
}
