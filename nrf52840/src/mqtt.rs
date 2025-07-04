use crate::{
    error::Error,
    send_punch::{Command, EVENT_CHANNEL, SendPunchMutexType},
};
use core::{marker::PhantomData, str::FromStr};
use defmt::{debug, error, info, warn};
use embassy_executor::Spawner;
use embassy_sync::semaphore::{FairSemaphore, Semaphore};
use embassy_time::{Duration, Instant, Timer, WithTimeout};
use heapless::{String, format};
use yaroc_common::{
    RawMutex,
    at::{
        mqtt::{MqttStatus, StatusCode},
        response::CommandResponse,
    },
    backoff::{
        BackoffCommand, BackoffRetries, CMD_FOR_BACKOFF, PUNCH_QUEUE_SIZE, PunchMsg, SendPunchFn,
    },
    bg77::hw::{ACTIVATION_TIMEOUT, ModemHw},
    punch::RawPunch,
};

const MQTT_CLIENT_ID: u8 = 0;
// Property of the Quectel BG77 hardware. Any more than 5 messages inflight fail to send.
const PUNCHES_INFLIGHT: usize = 5;

static MQTT_EXTRA_TIMEOUT: Duration = Duration::from_millis(300);
static BG77_PUNCH_SEMAPHORE: FairSemaphore<RawMutex, PUNCH_QUEUE_SIZE> =
    FairSemaphore::new(PUNCHES_INFLIGHT);

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
    pub name: String<20>,
    pub mac_address: String<12>,
    pub minicallhome_interval: Duration,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            url: String::from_str("broker.emqx.io").unwrap(),
            packet_timeout: Duration::from_secs(35),
            name: String::new(),
            mac_address: String::new(),
            minicallhome_interval: Duration::from_secs(30),
        }
    }
}

#[embassy_executor::task]
pub async fn backoff_retries_loop(mut backoff_retries: BackoffRetries<Bg77SendPunchFn>) {
    backoff_retries.r#loop().await;
}

#[derive(Clone, Copy)]
pub struct Bg77SendPunchFn {
    send_punch_mutex: &'static SendPunchMutexType,
    bg77_punch_semaphore: &'static FairSemaphore<RawMutex, PUNCH_QUEUE_SIZE>,
    packet_timeout: Duration,
}

impl Bg77SendPunchFn {
    pub fn new(send_punch_mutex: &'static SendPunchMutexType, packet_timeout: Duration) -> Self {
        Self {
            send_punch_mutex,
            bg77_punch_semaphore: &BG77_PUNCH_SEMAPHORE,
            packet_timeout,
        }
    }
}

#[embassy_executor::task(pool_size = PUNCH_QUEUE_SIZE)]
async fn bg77_send_punch_fn(
    msg: PunchMsg,
    send_punch_fn: Bg77SendPunchFn,
    send_punch_timeout: Duration,
) {
    BackoffRetries::<Bg77SendPunchFn>::try_sending_with_retries(
        msg,
        send_punch_fn,
        send_punch_timeout,
    )
    .await
}

impl SendPunchFn for Bg77SendPunchFn {
    type SemaphoreReleaser = embassy_sync::semaphore::SemaphoreReleaser<
        'static,
        FairSemaphore<RawMutex, PUNCH_QUEUE_SIZE>,
    >;

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

    async fn acquire(&mut self) -> crate::Result<Self::SemaphoreReleaser> {
        // The modem doesn't like too many messages being sent out at the same time.
        self.bg77_punch_semaphore.acquire(1).await.map_err(|_| Error::SemaphoreError)
    }

    fn spawn(self, msg: PunchMsg, spawner: Spawner, send_punch_timeout: Duration) {
        spawner.must_spawn(bg77_send_punch_fn(msg, self, send_punch_timeout));
    }
}

pub struct MqttClient<M: ModemHw> {
    config: MqttConfig,
    last_successful_send: Instant,
    cgatt_cnt: u8,
    punch_cnt: u16,
    _phantom: PhantomData<M>,
}

impl<M: ModemHw> MqttClient<M> {
    pub fn new(
        send_punch_mutex: &'static SendPunchMutexType,
        config: MqttConfig,
        spawner: Spawner,
    ) -> Self {
        let send_punch_for_backoff = Bg77SendPunchFn::new(send_punch_mutex, config.packet_timeout);
        let send_punch_timeout = ACTIVATION_TIMEOUT + config.packet_timeout * 2;
        let backoff_retries = BackoffRetries::new(
            send_punch_for_backoff,
            Duration::from_secs(10),
            send_punch_timeout,
            23,
        );
        spawner.must_spawn(backoff_retries_loop(backoff_retries));

        Self {
            config,
            last_successful_send: Instant::now(),
            cgatt_cnt: 0,
            punch_cnt: 0,
            _phantom: PhantomData,
        }
    }

    async fn network_registration(&mut self, bg77: &mut M) -> crate::Result<()> {
        if self.last_successful_send + self.config.packet_timeout * (4 + 2 * self.cgatt_cnt).into()
            < Instant::now()
        {
            warn!("Will reattach to network because of no messages being sent for a long time");
            self.last_successful_send = Instant::now();
            bg77.simple_call_at("E0", None).await?;
            let _ = bg77.call_at("+CGATT=0", ACTIVATION_TIMEOUT).await;
            Timer::after_secs(2).await;
            let _ = bg77.call_at("+CGACT=0,1", ACTIVATION_TIMEOUT).await;
            self.cgatt_cnt += 1;
        } else {
            let state = bg77.simple_call_at("+CGATT?", None).await?.parse1::<u8>([0], None)?;
            if state == 1 {
                info!("Already registered to network");
                return Ok(());
            }
        }

        bg77.call_at("+CGATT=1", ACTIVATION_TIMEOUT).await?;
        // CGATT=1 needs additional time and reading from modem
        Timer::after_secs(1).await;
        let response = bg77.read().await;
        if let Ok(response) = response {
            if !response.lines().is_empty() {
                debug!("Read {=[?]} after CGATT=1", response.lines());
            }
        }
        // TODO: should we do something with the result?
        let (_, _) =
            bg77.simple_call_at("+CGACT?", None).await?.parse2::<u8, u8>([0, 1], Some(1))?;

        Ok(())
    }

    pub fn urc_handler(response: &CommandResponse) -> bool {
        match response.command() {
            "QMTSTAT" | "QIURC" => {
                let message = Command::MqttConnect(true, Instant::now());
                if response.command() == "QMTSTAT"
                    && CMD_FOR_BACKOFF.try_send(BackoffCommand::MqttDisconnected).is_err()
                {
                    error!("Error while sending MQTT disconnect notification, channel full");
                }
                if EVENT_CHANNEL.try_send(message).is_err() {
                    error!("Error while sending MQTT connect command, channel full");
                }
                true
            }
            "CEREG" => response.values().len() == 4,
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
                info!("TCP connection already opened to {}", url);
                return Ok(());
            }
            warn!("Connected to the wrong broker {}, will disconnect", url);
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
            error!("Could not open TCP connection to {}", self.config.url);
            return Err(Error::MqttError(status));
        }

        Ok(())
    }

    pub async fn mqtt_connect(&mut self, bg77: &mut M) -> crate::Result<()> {
        self.network_registration(bg77)
            .await
            .inspect_err(|err| error!("Network registration failed: {}", err))?;
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
                let cmd = format!(50; "+QMTCONN={cid},\"nrf52840-{}\"", self.config.name)?;
                let (_, res, reason) = bg77
                    .simple_call_at(&cmd, Some(self.config.packet_timeout + MQTT_EXTRA_TIMEOUT))
                    .await?
                    .parse3::<u8, u32, i8>([0, 1, 2], Some(cid))?;

                if res == 0 && reason == 0 {
                    info!("Successfully connected to MQTT");
                    if CMD_FOR_BACKOFF.try_send(BackoffCommand::MqttConnected).is_err() {
                        error!("Error while sending MQTT connect notification, channel full");
                    }
                    self.cgatt_cnt = 0;
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
            "+QMTPUB={},{},{},0,\"yar/{}/{}\",{}", MQTT_CLIENT_ID, msg_id, qos as u8, &self.config.mac_address, topic, msg.len(),
        )?;
        bg77.simple_call_at(&cmd, None).await?;

        let second_read_timeout = if qos == MqttQos::Q0 {
            Some(Duration::from_secs(5))
        } else {
            None
        };
        let response = bg77.call(msg, "+QMTPUB", second_read_timeout).await?;
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
