use core::{marker::PhantomData, str::FromStr};
#[cfg(feature = "defmt")]
use defmt::{debug, error, info, warn};
use embassy_sync::channel::Sender;
use embassy_sync::lazy_lock::LazyLock;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use heapless::{String, format};
#[cfg(not(feature = "defmt"))]
use log::{error, info, warn};

use crate::{
    RawMutex,
    at::response::CommandResponse,
    backoff::{BackoffCommand, BatchedPunches, CMD_FOR_BACKOFF},
    bg77::hw::{ACTIVATION_TIMEOUT, ModemHw},
    error::Error,
    send_punch::SendPunchCommand,
};

static MQTT_EXTRA_TIMEOUT: Duration = Duration::from_millis(300);

pub static MQTT_MSG_PUBLISHED: LazyLock<[Signal<RawMutex, Instant>; 3]> =
    LazyLock::new(|| core::array::from_fn(|_| Signal::new()));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusCode {
    Published,
    Retrying(u8),
    Timeout,
    MqttError,
    Unknown,
}

#[derive(Debug, PartialEq, Eq)]
pub struct MqttStatus {
    pub msg_id: u16,
    pub code: StatusCode,
}

impl MqttStatus {
    pub fn from_bg77_qmtpub(msg_id: u16, status: u8, retries: Option<&u8>) -> Self {
        let status = match status {
            0 => StatusCode::Published,
            1 => StatusCode::Retrying(*retries.unwrap_or(&0)),
            2 => StatusCode::Timeout,
            _ => StatusCode::Unknown,
        };
        Self {
            msg_id,
            code: status,
        }
    }

    pub fn mqtt_error(msg_id: u16) -> Self {
        Self {
            msg_id,
            code: StatusCode::MqttError,
        }
    }
}

/// Quality of Service for MQTT messages.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MqttQos {
    /// At most once.
    Q0 = 0,
    /// At least once.
    Q1 = 1,
    // 2 is unsupported
}

/// Configuration for the MQTT client.
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

/// An MQTT client for the BG77 modem.
pub struct MqttClient<M: ModemHw> {
    config: MqttConfig,
    last_successful_send: Instant,
    client_id: u8,
    cgatt_cnt: u8,
    punch_cnt: u16,
    _phantom: PhantomData<M>,
}

impl<M: ModemHw> MqttClient<M> {
    /// Creates a new `MqttClient`.
    pub fn new(config: MqttConfig, client_id: u8) -> Self {
        Self {
            config,
            last_successful_send: Instant::now(),
            client_id,
            cgatt_cnt: 0,
            punch_cnt: 0,
            _phantom: PhantomData,
        }
    }

    /// Registers to the network.
    async fn network_registration(&mut self, bg77: &mut M) -> crate::Result<()> {
        if let Some(publish_time) = MQTT_MSG_PUBLISHED.get()[self.client_id as usize].try_take() {
            self.last_successful_send = self.last_successful_send.max(publish_time);
        }

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
        let _response = bg77.read().await;
        #[cfg(feature = "defmt")]
        if let Ok(response) = _response
            && !response.lines().is_empty()
        {
            debug!("Read {=[?]} after CGATT=1", response.lines());
        }
        // TODO: should we do something with the result?
        let (_, _) =
            bg77.simple_call_at("+CGACT?", None).await?.parse2::<u8, u8>([0, 1], Some(1))?;

        Ok(())
    }

    /// Handles URCs from the modem.
    pub fn urc_handler<const CLIENT_ID: u8>(
        response: &'_ CommandResponse,
        command_sender: Sender<'static, RawMutex, SendPunchCommand, 10>,
    ) -> bool {
        match response.command() {
            "QMTSTAT" | "QIURC" => {
                if response.command() == "QMTSTAT" {
                    warn!("MQTT disconnected");
                    if CMD_FOR_BACKOFF.try_send(BackoffCommand::MqttDisconnected).is_err() {
                        error!("Error while sending MQTT disconnect notification, channel full");
                    }
                }
                let message = SendPunchCommand::MqttConnect(true, Instant::now());
                if command_sender.try_send(message).is_err() {
                    error!("Error while sending MQTT connect command, channel full");
                }
                true
            }
            "CEREG" => response.values().len() == 4,
            "QMTPUB" => Self::qmtpub_handler::<CLIENT_ID>(response),
            _ => false,
        }
    }

    /// Handles the `+QMTPUB` URC.
    fn qmtpub_handler<const CLIENT_ID: u8>(response: &CommandResponse) -> bool {
        let values = match response.parse_values::<u8>() {
            Ok(values) => values,
            Err(_) => {
                return false;
            }
        };

        if values[0] == CLIENT_ID {
            let status = MqttStatus::from_bg77_qmtpub(values[1] as u16, values[2], values.get(3));
            if status.code == StatusCode::Published {
                MQTT_MSG_PUBLISHED.get()[usize::from(CLIENT_ID)].signal(Instant::now());
            }
            if status.msg_id > 0 {
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

    /// Opens a TCP connection to the MQTT broker.
    async fn mqtt_open(&self, bg77: &mut M) -> crate::Result<()> {
        let cid = self.client_id;
        let opened = bg77
            .simple_call_at("+QMTOPEN?", None)
            .await?
            .parse2::<u8, String<40>>([0, 1], Some(cid));
        if let Ok((client_id, url)) = opened
            && client_id == cid
        {
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

    /// Connects to the MQTT broker.
    pub async fn mqtt_connect(&mut self, bg77: &mut M) -> crate::Result<()> {
        self.network_registration(bg77)
            .await
            .inspect_err(|err| error!("Network registration failed: {}", err))?;
        self.mqtt_open(bg77).await?;

        let cid = self.client_id;
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

    /// Disconnects from the MQTT broker.
    #[allow(dead_code)]
    pub async fn mqtt_disconnect(&mut self, bg77: &mut M) -> Result<(), Error> {
        let cid = self.client_id;
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

    /// Sends a message to the MQTT broker.
    pub async fn send_message(
        &mut self,
        bg77: &mut M,
        topic: &str,
        msg: &[u8],
        qos: MqttQos,
        msg_id: u16,
    ) -> Result<(), Error> {
        let cid = self.client_id;
        let cmd = format!(100;
            "+QMTPUB={cid},{},{},0,\"yar/{}/{}\",{}",
            msg_id,
            qos as u8,
            &self.config.mac_address,
            topic,
            msg.len(),
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

    /// Schedules a batch of punches to be sent and returns its Punch ID.
    pub async fn schedule_punch(&mut self, punches: BatchedPunches) -> u16 {
        let punch_id = self.punch_cnt;
        CMD_FOR_BACKOFF.send(BackoffCommand::PublishPunches(punches, punch_id)).await;
        self.punch_cnt += 1;
        punch_id
    }
}

#[cfg(feature = "std")]
#[cfg(test)]
mod test {
    use super::*;
    use crate::bg77::hw::FakeModem;
    use embassy_futures::block_on;

    #[test]
    fn test_mqtt_disconnect_ok() {
        let mut bg77 = FakeModem::new(&[
            ("AT+QMTDISC=2", "+QMTDISC: 2,0"),
            ("AT+QMTCLOSE=2", "+QMTCLOSE: 2,0"),
        ]);

        let mut client = MqttClient::<_>::new(MqttConfig::default(), 2);
        assert_eq!(block_on(client.mqtt_disconnect(&mut bg77)), Ok(()));
    }
}
