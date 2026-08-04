use core::marker::PhantomData;
#[cfg(feature = "defmt")]
use defmt::{error, info, warn};
use embassy_sync::channel::Sender;
use embassy_sync::lazy_lock::LazyLock;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant};
use heapless::format;
#[cfg(not(feature = "defmt"))]
use log::{error, info, warn};

use crate::{
    RawMutex,
    at::{response::CommandResponse, uart::AtUartTrait},
    backoff::{BackoffCommand, CMD_FOR_BACKOFF},
    bg77::{connection::ConnectionEvent, modem_manager::ACTIVATION_TIMEOUT},
    error::Error,
    mqtt::{MqttClientConfig, MqttConfig, MqttQos, MqttStatus, StatusCode},
    send_punch::SendPunchCommand,
};

static MQTT_EXTRA_TIMEOUT: Duration = Duration::from_millis(300);

pub static MQTT_MSG_PUBLISHED: LazyLock<[Signal<RawMutex, Instant>; 3]> =
    LazyLock::new(|| core::array::from_fn(|_| Signal::new()));

#[derive(Debug, thiserror::Error, Clone, Copy, Eq, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum TcpError {
    #[error("Failed to open network")]
    FailedToOpenNetwork,
    #[error("Wrong parameter")]
    WrongParameter,
    #[error("Identifier occupied")]
    IdentifierOccupied,
    #[error("Failed to activate PDP context")]
    PdpContextFailed,
    #[error("Domain name parse error")]
    DomainNameError,
    #[error("Network disconnected")]
    NetworkDisconnected,
    #[error("Unknown TCP error ({0})")]
    Unknown(i8),
}

impl TcpError {
    pub fn from_code(status: i8) -> Self {
        match status {
            -1 => Self::FailedToOpenNetwork,
            1 => Self::WrongParameter,
            2 => Self::IdentifierOccupied,
            3 => Self::PdpContextFailed,
            4 => Self::DomainNameError,
            5 => Self::NetworkDisconnected,
            code => Self::Unknown(code),
        }
    }
}

#[derive(Debug, thiserror::Error, Clone, Copy, Eq, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ConnectError {
    #[error("Packet retransmission timeout")]
    RetransmissionTimeout,
    #[error("Failed to send packet")]
    PacketSendFailed,
    #[error("Connection refused: unacceptable protocol version")]
    UnacceptableProtocolVersion,
    #[error("Connection refused: identifier rejected")]
    IdentifierRejected,
    #[error("Connection refused: server unavailable")]
    ServerUnavailable,
    #[error("Connection refused: bad user name or password")]
    BadUsernameOrPassword,
    #[error("Connection refused: not authorized")]
    NotAuthorized,
    #[error("Unknown connect error ({0})")]
    Unknown(u8),
}

impl ConnectError {
    pub fn from_code(res: u8, reason: u8) -> Self {
        if reason != 0 {
            match reason {
                1 => Self::UnacceptableProtocolVersion,
                2 => Self::IdentifierRejected,
                3 => Self::ServerUnavailable,
                4 => Self::BadUsernameOrPassword,
                5 => Self::NotAuthorized,
                _ => Self::Unknown(reason),
            }
        } else {
            match res {
                1 => Self::RetransmissionTimeout,
                2 => Self::PacketSendFailed,
                code => Self::Unknown(code),
            }
        }
    }
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
}

/// An MQTT client for the BG77 modem.
pub struct MqttClient<M: AtUartTrait> {
    config: MqttClientConfig,
    last_successful_send: Instant,
    client_id: u8,
    _phantom: PhantomData<M>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
/// Status of the +QMTCONN AT command.
enum QmtconnStatus {
    Initializing = 1,
    Connecting = 2,
    Connected = 3,
    Disconnecting = 4,
}

impl TryFrom<u8> for QmtconnStatus {
    type Error = Error;

    fn try_from(val: u8) -> crate::Result<Self> {
        match val {
            1 => Ok(Self::Initializing),
            2 => Ok(Self::Connecting),
            3 => Ok(Self::Connected),
            4 => Ok(Self::Disconnecting),
            _ => Err(Error::ValueError),
        }
    }
}

impl<M: AtUartTrait> MqttClient<M> {
    /// Creates a new `MqttClient`.
    pub fn new(config: MqttClientConfig, client_id: u8) -> Self {
        Self {
            config,
            last_successful_send: Instant::now(),
            client_id,
            _phantom: PhantomData,
        }
    }

    /// Updates the MQTT client configuration.
    pub fn update_config(&mut self, config: MqttClientConfig) {
        self.config = config;
    }

    /// Updates the MQTT client configuration from a reduced configuration.
    pub fn update_reduced_config(&mut self, reduced: MqttConfig) {
        self.config.update(reduced);
    }

    /// Returns the time of the last successful sent message
    pub fn last_successful_send(&mut self) -> Instant {
        if let Some(publish_time) = MQTT_MSG_PUBLISHED.get()[self.client_id as usize].try_take() {
            self.last_successful_send = self.last_successful_send.max(publish_time);
        }
        self.last_successful_send
    }

    /// Returns the configured value of packet timeout
    pub fn packet_timeout(&self) -> Duration {
        self.config.packet_timeout
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
                if CMD_FOR_BACKOFF.try_send(BackoffCommand::MqttDisconnected).is_err() {
                    error!("Channel full when sending MQTT disconnect notification");
                }
                let message =
                    SendPunchCommand::ConnectionSupervisorEvent(ConnectionEvent::MqttDisconnect(1));
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
            .parse3::<u8, heapless::String<40>, u16>([0, 1, 2], Some(cid));
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

        let cmd = format!(100;
            "+QMTCFG=\"will\",{cid},1,1,0,\"yar/{}/will\",\"{}\"",
            self.config.mac_address, self.config.name
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
            return Err(TcpError::from_code(status).into());
        }

        Ok(())
    }

    /// Checks whether the MQTT client is connected
    pub async fn is_connected(&self, bg77: &mut M) -> crate::Result<bool> {
        let (_, status) = bg77
            .call_at("+QMTCONN?", None)
            .await?
            .parse2::<u8, u8>([0, 1], Some(self.client_id))?;
        let status = QmtconnStatus::try_from(status).map_err(|_| Error::ModemError)?;
        Ok(status == QmtconnStatus::Connected)
    }

    /// Connects to the MQTT broker.
    ///
    /// This function first ensures network registration and then opens a TCP connection
    /// using `Self::open()`. Finally, it attempts to connect to the MQTT broker.
    pub async fn connect(&mut self, bg77: &mut M) -> crate::Result<()> {
        let cid = self.client_id;
        self.open(bg77).await?;

        let (_, status) =
            bg77.call_at("+QMTCONN?", None).await?.parse2::<u8, u8>([0, 1], Some(cid))?;
        let status = QmtconnStatus::try_from(status).map_err(|_| Error::ModemError)?;
        match status {
            QmtconnStatus::Connected => {
                info!("Already connected to MQTT");
                Ok(())
            }
            QmtconnStatus::Disconnecting | QmtconnStatus::Connecting => {
                info!("Connecting or disconnecting from MQTT in progress");
                Ok(())
            }
            QmtconnStatus::Initializing => {
                info!("Will connect to MQTT");
                let cmd = match &self.config.credentials {
                    Some((username, password)) => {
                        format!(100; "+QMTCONN={cid},\"{}\",\"{username}\",\"{password}\"", self.config.name)?
                    }
                    None => format!(100; "+QMTCONN={cid},\"{}\"", self.config.name)?,
                };
                let (_client_id, res, reason) = bg77
                    .call_at(&cmd, Some(self.config.packet_timeout + MQTT_EXTRA_TIMEOUT))
                    .await?
                    .parse3::<u8, u8, u8>([0, 1, 2], Some(cid))?;

                if res == 0 && reason == 0 {
                    info!("Successfully connected to MQTT");
                    if CMD_FOR_BACKOFF.try_send(BackoffCommand::MqttConnected).is_err() {
                        error!("Error while sending MQTT connect notification, channel full");
                    }
                    self.last_successful_send = Instant::now();
                    Ok(())
                } else {
                    Err(ConnectError::from_code(res, reason).into())
                }
            }
        }
    }

    /// Close the MQTT connection to the MQTT broker.
    pub async fn disconnect(&self, bg77: &mut M) -> Result<(), Error> {
        let cid = self.client_id;
        // TODO: query the current connection status, rather than calling a redundant command.
        let response = bg77.call_at("+QMTCONN?", None).await?.parse2::<u8, u8>([0, 1], Some(cid));
        if let Ok((_, status)) = response
            && Ok(QmtconnStatus::Connected) == QmtconnStatus::try_from(status)
        {
            let cmd = format!(50; "+QMTCLOSE={cid}")?;
            bg77.call_at(&cmd, Some(ACTIVATION_TIMEOUT)).await?;
        }
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
                // TODO: forward the actual error, if possible
                StatusCode::MqttError => Err(ConnectError::Unknown(0).into()),
                StatusCode::Unknown => Err(ConnectError::Unknown(1).into()),
            }
        } else {
            Ok(())
        }
    }
}

#[cfg(feature = "std")]
#[cfg(test)]
mod test {
    use super::*;
    use crate::at::fake_modem::FakeModem;
    use core::str::FromStr;
    use embassy_futures::block_on;
    use embassy_sync::channel::Channel;
    use embassy_sync::mutex::Mutex;
    use heapless::String;
    static CHANNEL: Channel<RawMutex, SendPunchCommand, 10> = Channel::new();
    static CHANNEL_MUTEX: Mutex<RawMutex, ()> = Mutex::new(());

    #[test]
    fn test_mqtt_wrong_broker_disconnects_first() {
        let _lock = block_on(CHANNEL_MUTEX.lock());
        let client_config = MqttClientConfig {
            url: String::from_str("correct.broker.io").unwrap(),
            name: String::from_str("test_client").unwrap(),
            ..Default::default()
        };

        let mut bg77 = FakeModem::new(&[
            ("AT+QMTOPEN?", "+QMTOPEN: 1,\"wrong.broker.io\",1883"), // Connected to wrong broker
            ("AT+QMTCONN?", "+QMTCONN: 1,3"),
            ("AT+QMTCLOSE=1", "+QMTCLOSE: 1,0"), // Disconnect from wrong broker
            ("AT+QMTCFG=\"timeout\",1,35,2,1", "+QMTCFG: 1,0"),
            ("AT+QMTCFG=\"keepalive\",1,70", "+QMTCFG: 1,0"),
            (
                "AT+QMTCFG=\"will\",1,1,1,0,\"yar/deadbeef/will\",\"test_client\"",
                "+QMTCFG: 1,0",
            ),
            ("AT+QMTOPEN=1,\"correct.broker.io\",1883", "+QMTOPEN: 1,0"),
            ("AT+QMTCONN?", "+QMTCONN: 1,1"),
            ("AT+QMTCONN=1,\"test_client\"", "+QMTCONN: 1,0,0"),
        ]);

        let mut client = MqttClient::<_>::new(client_config, 1);
        assert_eq!(block_on(client.connect(&mut bg77)), Ok(()));
        assert!(bg77.all_done());
    }

    #[test]
    fn test_mqtt_custom_port() {
        let _lock = block_on(CHANNEL_MUTEX.lock());
        let client_config = MqttClientConfig {
            port: 8883,
            name: String::from_str("test_client").unwrap(),
            ..Default::default()
        };

        let mut bg77 = FakeModem::new(&[
            ("AT+QMTOPEN?", "+QMTOPEN: 1,\"broker.emqx.io\",8883"), // Already connected to correct port
            ("AT+QMTCONN?", "+QMTCONN: 1,3"),
        ]);

        let mut client = MqttClient::<_>::new(client_config, 1);
        assert_eq!(block_on(client.connect(&mut bg77)), Ok(()));
        assert!(bg77.all_done());
    }

    #[test]
    fn test_mqtt_already_connected() {
        let _lock = block_on(CHANNEL_MUTEX.lock());
        let mut bg77 = FakeModem::new(&[
            ("AT+QMTOPEN?", "+QMTOPEN: 1,\"broker.emqx.io\",1883"),
            ("AT+QMTCONN?", "+QMTCONN: 1,3"),
        ]);

        let mut client = MqttClient::<_>::new(MqttClientConfig::default(), 1);
        assert_eq!(block_on(client.connect(&mut bg77)), Ok(()));
        assert!(bg77.all_done());
    }

    #[test]
    fn test_mqtt_disconnect_ok() {
        let _lock = block_on(CHANNEL_MUTEX.lock());
        let mut bg77 = FakeModem::new(&[
            ("AT+QMTCONN?", "+QMTCONN: 2,3"),
            ("AT+QMTCLOSE=2", "+QMTCLOSE: 2,0"),
        ]);

        let client = MqttClient::<_>::new(MqttClientConfig::default(), 2);
        assert_eq!(block_on(client.disconnect(&mut bg77)), Ok(()));
        assert!(bg77.all_done());
    }

    #[test]
    fn test_mqtt_send_ok() {
        let _lock = block_on(CHANNEL_MUTEX.lock());
        let mut bg77 = FakeModem::new(&[("AT+QMTPUB=2,0,0,0,\"yar/deadbeef/tpc\",1", "")]);
        bg77.add_pure_interactions(&[("+QMTPUB", true, "+QMTPUB: 2,0,0")]);
        let mut client = MqttClient::<_>::new(MqttClientConfig::default(), 2);
        let res = block_on(client.send_message(&mut bg77, "tpc", &[47], MqttQos::Q0, 0));
        assert_eq!(res, Ok(()));
        assert!(bg77.all_done());
    }

    #[test]
    fn test_mqtt_send_timeout() {
        let _lock = block_on(CHANNEL_MUTEX.lock());
        let mut bg77 = FakeModem::new(&[("AT+QMTPUB=2,0,0,0,\"yar/deadbeef/tpc\",1", "")]);
        bg77.add_pure_interactions(&[("+QMTPUB", true, "+QMTPUB: 2,0,2")]);
        let mut client = MqttClient::<_>::new(MqttClientConfig::default(), 2);
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

    #[test]
    fn test_mqtt_qmtopen_tcp_error() {
        let _lock = block_on(CHANNEL_MUTEX.lock());
        let client_config = MqttClientConfig::default();

        let mut bg77 = FakeModem::new(&[
            ("AT+QMTOPEN?", ""),
            ("AT+QMTCFG=\"timeout\",1,35,2,1", "+QMTCFG: 1,0"),
            ("AT+QMTCFG=\"keepalive\",1,70", "+QMTCFG: 1,0"),
            (
                "AT+QMTCFG=\"will\",1,1,1,0,\"yar/deadbeef/will\",\"test_client\"",
                "+QMTCFG: 1,0",
            ),
            ("AT+QMTOPEN=1,\"broker.emqx.io\",1883", "+QMTOPEN: 1,-1"),
        ]);

        let mut client = MqttClient::<_>::new(client_config, 1);
        assert_eq!(
            block_on(client.connect(&mut bg77)),
            Err(Error::MqttTcp(TcpError::FailedToOpenNetwork))
        );
        assert!(bg77.all_done());
    }

    #[test]
    fn test_mqtt_qmtopen_domain_name_error() {
        let _lock = block_on(CHANNEL_MUTEX.lock());
        let client_config = MqttClientConfig::default();

        let mut bg77 = FakeModem::new(&[
            ("AT+QMTOPEN?", ""),
            ("AT+QMTCFG=\"timeout\",1,35,2,1", "+QMTCFG: 1,0"),
            ("AT+QMTCFG=\"keepalive\",1,70", "+QMTCFG: 1,0"),
            (
                "AT+QMTCFG=\"will\",1,1,1,0,\"yar/deadbeef/will\",\"test_client\"",
                "+QMTCFG: 1,0",
            ),
            ("AT+QMTOPEN=1,\"broker.emqx.io\",1883", "+QMTOPEN: 1,4"),
        ]);

        let mut client = MqttClient::<_>::new(client_config, 1);
        assert_eq!(
            block_on(client.connect(&mut bg77)),
            Err(Error::MqttTcp(TcpError::DomainNameError))
        );
        assert!(bg77.all_done());
    }

    #[test]
    fn test_mqtt_qmtconn_mqtt_error() {
        let _lock = block_on(CHANNEL_MUTEX.lock());
        let client_config = MqttClientConfig::default();

        let mut bg77 = FakeModem::new(&[
            ("AT+QMTOPEN?", "+QMTOPEN: 1,\"broker.emqx.io\",1883"),
            ("AT+QMTCONN?", "+QMTCONN: 1,1"),
            ("AT+QMTCONN=1,\"test_client\"", "+QMTCONN: 1,0,2"),
        ]);

        let mut client = MqttClient::<_>::new(client_config, 1);
        assert_eq!(
            block_on(client.connect(&mut bg77)),
            Err(Error::MqttConnect(ConnectError::IdentifierRejected))
        );
        assert!(bg77.all_done());
    }

    #[test]
    fn test_mqtt_qmtconn_retransmission_timeout() {
        let _lock = block_on(CHANNEL_MUTEX.lock());
        let client_config = MqttClientConfig::default();

        let mut bg77 = FakeModem::new(&[
            ("AT+QMTOPEN?", "+QMTOPEN: 1,\"broker.emqx.io\",1883"),
            ("AT+QMTCONN?", "+QMTCONN: 1,1"),
            ("AT+QMTCONN=1,\"test_client\"", "+QMTCONN: 1,1,0"),
        ]);

        let mut client = MqttClient::<_>::new(client_config, 1);
        assert_eq!(
            block_on(client.connect(&mut bg77)),
            Err(Error::MqttConnect(ConnectError::RetransmissionTimeout))
        );
        assert!(bg77.all_done());
    }
}
