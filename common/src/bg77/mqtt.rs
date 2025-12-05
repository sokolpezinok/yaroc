use core::{marker::PhantomData, str::FromStr};
#[cfg(feature = "defmt")]
use defmt::{error, info, warn};
use embassy_sync::channel::Sender;
use embassy_sync::lazy_lock::LazyLock;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant};
use heapless::{String, format};
#[cfg(not(feature = "defmt"))]
use log::{error, info, warn};

use crate::{
    RawMutex,
    at::response::CommandResponse,
    backoff::{BackoffCommand, BatchedPunches, CMD_FOR_BACKOFF},
    bg77::{
        hw::ModemHw,
        modem_manager::{ACTIVATION_TIMEOUT, ModemManager},
    },
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

/// Represents the status of an MQTT message publication.
#[derive(Debug, PartialEq, Eq)]
pub struct MqttStatus {
    pub msg_id: u16,
    pub code: StatusCode,
}

impl MqttStatus {
    /// Creates an `MqttStatus` from a BG77 `+QMTPUB` URC.
    ///
    /// `msg_id` is the message ID.
    /// `status` is the status code reported by the modem (0: Published, 1: Retrying, 2: Timeout).
    /// `retries` is an optional number of retries if the status is `Retrying`.
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

    /// Creates an `MqttStatus` indicating an MQTT error.
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

/// Configuration for the MQTT client to connect to a broker.
#[derive(Clone, Debug)]
pub struct MqttConfig {
    /// The URL of the MQTT broker, e.g., "broker.emqx.io".
    pub url: String<40>,
    /// Optional login credentials for the MQTT broker, username and password.
    pub credentials: Option<(String<20>, String<30>)>,
    /// The timeout duration for individual MQTT packets.
    pub packet_timeout: Duration,
    /// The name of the client, used to construct the MQTT client ID.
    pub name: String<20>,
    /// The MAC address of the device, used to form MQTT topics (e.g., "yar/mac_address/topic").
    pub mac_address: String<12>,
    /// The interval at which mini call home messages are sent.
    pub minicallhome_interval: Duration,
    /// The port of the MQTT broker.
    pub port: u16,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            url: String::from_str("broker.emqx.io").unwrap(),
            credentials: None,
            packet_timeout: Duration::from_secs(35),
            name: String::new(),
            mac_address: String::from_str("deadbeef").unwrap(),
            minicallhome_interval: Duration::from_secs(30),
            port: 1883,
        }
    }
}

/// An MQTT client for the BG77 modem.
pub struct MqttClient<M: ModemHw> {
    config: MqttConfig,
    last_successful_send: Instant,
    client_id: u8,
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
            punch_cnt: 0,
            _phantom: PhantomData,
        }
    }

    /// Handles Unsolicited Result Codes (URCs) from the modem.
    ///
    /// This function processes various URCs such as `QMTSTAT`, `QIURC`, `CEREG`, and `QMTPUB`.
    /// It sends appropriate `BackoffCommand`s or `SendPunchCommand`s based on the URC received.
    ///
    /// Returns `true` if the URC was handled, `false` otherwise.
    pub fn urc_handler<const CLIENT_ID: u8>(
        response: &'_ CommandResponse,
        command_sender: Sender<'static, RawMutex, SendPunchCommand, 10>,
    ) -> bool {
        match response.command() {
            "QMTSTAT" => {
                warn!("MQTT disconnected");
                if CMD_FOR_BACKOFF.try_send(BackoffCommand::MqttDisconnected).is_err() {
                    error!("Channel full when sending MQTT disconnect notification");
                }
                let message = SendPunchCommand::MqttConnect(true, Instant::now());
                if command_sender.try_send(message).is_err() {
                    error!("Error while sending MQTT connect command, channel full");
                }
                true
            }
            "QMTPUB" => Self::qmtpub_handler::<CLIENT_ID>(response),
            _ => false,
        }
    }

    /// Handles the `+QMTPUB` URC, which indicates the status of an MQTT message publication.
    ///
    /// If the message is successfully published, it signals `MQTT_MSG_PUBLISHED`.
    /// It also sends a `BackoffCommand::Status` to the backoff task.
    ///
    /// Returns `true` if the URC was handled for the given `CLIENT_ID`, `false` otherwise.
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
                // Message ID 0 is for QoS level 0, it's not handled as URC.
                false
            }
        } else {
            false
        }
    }

    /// Opens a TCP connection to the configured MQTT broker.
    ///
    /// If a connection is already open to the correct broker, it does nothing.
    /// If connected to a different broker, it disconnects first.
    /// It also configures MQTT timeouts and keep-alive settings before opening the connection.
    async fn open(&self, bg77: &mut M) -> crate::Result<()> {
        let cid = self.client_id;
        let opened = bg77
            .call_at("+QMTOPEN?", None)
            .await?
            .parse3::<u8, String<40>, u16>([0, 1, 2], Some(cid));
        if let Ok((client_id, url, port)) = opened
            && client_id == cid
        {
            if *url == self.config.url && port == self.config.port {
                info!("TCP connection already opened to {}:{}", url, port);
                return Ok(());
            }
            warn!(
                "Connected to the wrong broker {}:{}, will disconnect",
                url, port
            );
            self.disconnect(bg77).await?;
        }

        let cmd = format!(50;
            "+QMTCFG=\"timeout\",{cid},{},2,1",
            self.config.packet_timeout.as_secs()
        )?;
        bg77.call_at(&cmd, None).await?;
        let cmd = format!(50;
            "+QMTCFG=\"keepalive\",{cid},{}",
            (self.config.packet_timeout * 2).as_secs()
        )?;
        bg77.call_at(&cmd, None).await?;

        let cmd = format!(100; "+QMTOPEN={cid},\"{}\",{}", self.config.url, self.config.port)?;
        let (_, status) = bg77
            .call_at(&cmd, Some(ACTIVATION_TIMEOUT))
            .await?
            .parse2::<u8, i8>([0, 1], Some(cid))?;
        if status != 0 {
            error!(
                "Could not open TCP connection to {}:{}",
                self.config.url, self.config.port
            );
            return Err(Error::MqttError(status));
        }

        Ok(())
    }

    /// Connects to the MQTT broker.
    ///
    /// This function first ensures network registration and then opens a TCP connection
    /// using `Self::open()`. Finally, it attempts to connect to the MQTT broker.
    pub async fn connect(
        &mut self,
        bg77: &mut M,
        modem_manager: &ModemManager<M>,
    ) -> crate::Result<()> {
        let cid = self.client_id;
        if let Some(publish_time) = MQTT_MSG_PUBLISHED.get()[cid as usize].try_take() {
            self.last_successful_send = self.last_successful_send.max(publish_time);
        }
        let force_reattach =
            self.last_successful_send + self.config.packet_timeout * 4 < Instant::now();

        modem_manager
            .network_registration(bg77, force_reattach)
            .await
            .inspect_err(|err| error!("Network registration failed: {}", err))?;
        if force_reattach {
            self.last_successful_send = Instant::now();
        }
        self.open(bg77).await?;

        let (_, status) =
            bg77.call_at("+QMTCONN?", None).await?.parse2::<u8, u8>([0, 1], Some(cid))?;
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
                info!("Connecting or disconnecting from MQTT in progress");
                Ok(())
            }
            MQTT_INITIALIZING => {
                info!("Will connect to MQTT");
                let cmd = match &self.config.credentials {
                    Some((username, password)) => {
                        format!(100; "+QMTCONN={cid},\"nrf52840-{}\",\"{username}\",\"{password}\"", self.config.name)?
                    }
                    None => format!(100; "+QMTCONN={cid},\"nrf52840-{}\"", self.config.name)?,
                };
                let (_, res, reason) = bg77
                    .call_at(&cmd, Some(self.config.packet_timeout + MQTT_EXTRA_TIMEOUT))
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

    /// Close the MQTT connection to the MQTT broker.
    pub async fn disconnect(&self, bg77: &mut M) -> Result<(), Error> {
        let cid = self.client_id;
        let cmd = format!(50; "+QMTCLOSE={cid}")?;
        bg77.call_at(&cmd, Some(ACTIVATION_TIMEOUT)).await?;
        Ok(())
    }

    /// Sends a message to the MQTT broker on the specified topic with the given Quality of Service (QoS).
    ///
    /// `topic` is the MQTT topic to publish to.
    /// `msg` is the payload of the message.
    /// `qos` is the Quality of Service level.
    /// `msg_id` is the message ID.
    ///
    /// For QoS 0, it updates `last_successful_send` immediately upon publication.
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
        bg77.call_at(&cmd, None).await?;

        let second_read_timeout = if qos == MqttQos::Q0 {
            // The response is usually very quick, but we set a longer timeout just in case
            Some(self.config.packet_timeout)
        } else {
            None
        };
        let response = bg77.call(msg, "+QMTPUB", second_read_timeout).await?;
        if qos == MqttQos::Q0 {
            let (msg_id, status) = response.parse2::<u16, u8>([1, 2], None)?;
            let status = MqttStatus::from_bg77_qmtpub(msg_id, status, None);
            match status.code {
                StatusCode::Published => {
                    self.last_successful_send = Instant::now();
                    Ok(())
                }
                StatusCode::Retrying(_) => Ok(()),
                StatusCode::Timeout => Err(Error::TimeoutError),
                StatusCode::MqttError => Err(Error::MqttError(0)),
                StatusCode::Unknown => Err(Error::MqttError(-1)),
            }
        } else {
            Ok(())
        }
    }

    /// Schedules a batch of punches to be sent via the backoff mechanism.
    ///
    /// Returns the assigned punch ID for the scheduled batch.
    pub async fn schedule_punches(&mut self, punches: BatchedPunches) -> u16 {
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
    use crate::at::fake_modem::FakeModem;
    use crate::bg77::modem_manager::ModemConfig;
    use embassy_futures::block_on;
    use embassy_sync::channel::Channel;
    use embassy_sync::mutex::Mutex;
    static CHANNEL: Channel<RawMutex, SendPunchCommand, 10> = Channel::new();
    static CHANNEL_MUTEX: Mutex<RawMutex, ()> = Mutex::new(());

    #[test]
    fn test_mqtt_wrong_broker_disconnects_first() {
        let mut client_config = MqttConfig::default();
        client_config.url = String::from_str("correct.broker.io").unwrap();
        client_config.name = String::from_str("test_client").unwrap();

        let mut bg77 = FakeModem::new(&[
            ("AT+CGATT?", "+CGATT: 1"),
            ("AT+QMTOPEN?", "+QMTOPEN: 1,\"wrong.broker.io\",1883"), // Connected to wrong broker
            ("AT+QMTCLOSE=1", "+QMTCLOSE: 1,0"),                     // Disconnect from wrong broker
            ("AT+QMTCFG=\"timeout\",1,35,2,1", "+QMTCFG: 1,0"),
            ("AT+QMTCFG=\"keepalive\",1,70", "+QMTCFG: 1,0"),
            ("AT+QMTOPEN=1,\"correct.broker.io\",1883", "+QMTOPEN: 1,0"),
            ("AT+QMTCONN?", "+QMTCONN: 1,1"),
            ("AT+QMTCONN=1,\"nrf52840-test_client\"", "+QMTCONN: 1,0,0"),
        ]);

        let mut client = MqttClient::<_>::new(client_config, 1);
        let modem_manager = ModemManager::new(ModemConfig::default());
        assert_eq!(block_on(client.connect(&mut bg77, &modem_manager)), Ok(()));
        assert!(bg77.all_done());
    }

    #[test]
    fn test_mqtt_custom_port() {
        let mut client_config = MqttConfig::default();
        client_config.port = 8883;
        client_config.name = String::from_str("test_client").unwrap();

        let mut bg77 = FakeModem::new(&[
            ("AT+CGATT?", "+CGATT: 1"),
            ("AT+QMTOPEN?", "+QMTOPEN: 1,\"broker.emqx.io\",8883"), // Already connected to correct port
            ("AT+QMTCONN?", "+QMTCONN: 1,3"),
        ]);

        let mut client = MqttClient::<_>::new(client_config, 1);
        let modem_manager = ModemManager::new(ModemConfig::default());
        assert_eq!(block_on(client.connect(&mut bg77, &modem_manager)), Ok(()));
        assert!(bg77.all_done());
    }

    #[test]
    fn test_mqtt_already_connected() {
        let mut bg77 = FakeModem::new(&[
            ("AT+CGATT?", "+CGATT: 1"),
            ("AT+QMTOPEN?", "+QMTOPEN: 1,\"broker.emqx.io\",1883"),
            ("AT+QMTCONN?", "+QMTCONN: 1,3"),
        ]);

        let mut client = MqttClient::<_>::new(MqttConfig::default(), 1);
        let modem_manager = ModemManager::new(ModemConfig::default());
        assert_eq!(block_on(client.connect(&mut bg77, &modem_manager)), Ok(()));
        assert!(bg77.all_done());
    }

    #[test]
    fn test_mqtt_disconnect_ok() {
        let mut bg77 = FakeModem::new(&[("AT+QMTCLOSE=2", "+QMTCLOSE: 2,0")]);

        let client = MqttClient::<_>::new(MqttConfig::default(), 2);
        assert_eq!(block_on(client.disconnect(&mut bg77)), Ok(()));
        assert!(bg77.all_done());
    }

    #[test]
    fn test_mqtt_send_ok() {
        let mut bg77 = FakeModem::new(&[("AT+QMTPUB=2,0,0,0,\"yar/deadbeef/tpc\",1", "")]);
        bg77.add_pure_interactions(&[("+QMTPUB", true, "+QMTPUB: 2,0,0")]);
        let mut client = MqttClient::<_>::new(MqttConfig::default(), 2);
        let res = block_on(client.send_message(&mut bg77, "tpc", &[47], MqttQos::Q0, 0));
        assert_eq!(res, Ok(()));
        assert!(bg77.all_done());
    }

    #[test]
    fn test_mqtt_send_timeout() {
        let mut bg77 = FakeModem::new(&[("AT+QMTPUB=2,0,0,0,\"yar/deadbeef/tpc\",1", "")]);
        bg77.add_pure_interactions(&[("+QMTPUB", true, "+QMTPUB: 2,0,2")]);
        let mut client = MqttClient::<_>::new(MqttConfig::default(), 2);
        let res = block_on(client.send_message(&mut bg77, "tpc", &[47], MqttQos::Q0, 0));
        assert_eq!(res, Err(Error::TimeoutError));
        assert!(bg77.all_done());
    }

    #[test]
    fn test_qmtpub_handler_published() {
        // Client ID 0, Message ID 1, Status 0 (Published), Retries 0
        let response = CommandResponse::new("+QMTPUB: 0,1,0").unwrap();
        let _lock = block_on(CHANNEL_MUTEX.lock());
        let sender = CHANNEL.sender();

        // Ensure the signal is not set initially
        MQTT_MSG_PUBLISHED.get()[0].reset();
        while CMD_FOR_BACKOFF.try_receive().is_ok() {}

        let handled = MqttClient::<FakeModem>::urc_handler::<0>(&response, sender);
        assert!(handled);
        assert!(MQTT_MSG_PUBLISHED.get()[0].try_take().is_some());

        let expected_status = MqttStatus {
            msg_id: 1,
            code: StatusCode::Published,
        };
        let status = CMD_FOR_BACKOFF.try_receive().unwrap();
        assert_eq!(status, BackoffCommand::Status(expected_status));

        // The same as above, but for client ID 1
        let response = CommandResponse::new("+QMTPUB: 1,1,0").unwrap();
        // Return false, because it's for a different client
        let handled = MqttClient::<FakeModem>::urc_handler::<0>(&response, sender);
        assert!(!handled);
        assert!(MQTT_MSG_PUBLISHED.get()[0].try_take().is_none());
        assert!(CMD_FOR_BACKOFF.try_receive().is_err());
        assert!(CHANNEL.try_receive().is_err());
    }

    #[test]
    fn test_qmtpub_handler_timeout() {
        // Client ID 0, Message ID 2, Status 2 (Timeout)
        let response = CommandResponse::new("+QMTPUB: 0,2,2").unwrap();
        // Lock the channel, so that the concurrent tests do not interfere with each other
        let _lock = block_on(CHANNEL_MUTEX.lock());
        let sender = CHANNEL.sender();

        MQTT_MSG_PUBLISHED.get()[0].reset();
        while CMD_FOR_BACKOFF.try_receive().is_ok() {}

        let handled = MqttClient::<FakeModem>::urc_handler::<0>(&response, sender);
        assert!(handled);
        assert!(MQTT_MSG_PUBLISHED.get()[0].try_take().is_none());

        let expected_status = MqttStatus {
            msg_id: 2,
            code: StatusCode::Timeout,
        };
        let status = CMD_FOR_BACKOFF.try_receive().unwrap();
        assert_eq!(status, BackoffCommand::Status(expected_status));
        assert!(CHANNEL.try_receive().is_err());
    }
}
