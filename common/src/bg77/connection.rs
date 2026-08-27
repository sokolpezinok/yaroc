use core::marker::PhantomData;
use embassy_sync::watch::Watch;
use embassy_time::{Duration, Instant};

#[cfg(feature = "defmt")]
use defmt::{Format, error, info, warn};
#[cfg(not(feature = "defmt"))]
use log::{error, info, warn};

use crate::RawMutex;
use crate::at::uart::AtUartTrait;
use crate::bg77::modem_manager::ModemManager;
use crate::bg77::mqtt::{ConnectError, MqttClient, TcpError};

const MAX_INACTIVE_FORCE_REATTACH: Duration = Duration::from_secs(210);
const INITIAL_FORCE_REATTACH_RATE_LIMIT: Duration = Duration::from_secs(210);
const MAX_FORCE_REATTACH_RATE_LIMIT: Duration = Duration::from_secs(45 * 60);

pub static MQTT_CONNECTION_STATUS: Watch<RawMutex, bool, 1> = Watch::new();
pub static CELLULAR_CONNECTION_STATUS: Watch<RawMutex, bool, 1> = Watch::new();

/// Explicit state of the cellular and MQTT connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(Format))]
pub enum ConnectionState {
    /// Completely disconnected from cellular network and MQTT broker.
    #[default]
    Disconnected,
    /// Registering to cellular network (AT+CGATT / AT+CGACT).
    ConnectingCellular,
    /// Cellular registration failed
    CellularRegistrationFailed,
    /// Opening TCP socket and establishing MQTT session (+QMTOPEN / +QMTCONN).
    ConnectingMqtt,
    /// Opening TCP socket failed (+QMTOPEN failed).
    TcpError(TcpError),
    /// MQTT connection handshaking failed (+QMTCONN failed).
    ConnectError(ConnectError),
    /// Fully connected to MQTT broker and ready to transmit payloads.
    MqttConnected,
}

impl ConnectionState {
    /// Returns `true` if currently connected to MQTT.
    #[inline]
    pub fn is_mqtt_connected(&self) -> bool {
        matches!(self, Self::MqttConnected)
    }

    #[inline]
    pub fn is_mqtt_error(&self) -> bool {
        matches!(
            self,
            ConnectionState::TcpError(_) | ConnectionState::ConnectError(_)
        )
    }

    /// Returns `true` if currently connected to the cellular network.
    #[inline]
    pub fn is_cellular_connected(&self) -> bool {
        !matches!(
            self,
            Self::Disconnected | Self::ConnectingCellular | Self::CellularRegistrationFailed
        )
    }

    /// Returns `true` if currently in any connecting phase.
    #[inline]
    pub fn is_connecting(&self) -> bool {
        matches!(self, Self::ConnectingCellular | Self::ConnectingMqtt)
    }
}

impl From<TcpError> for ConnectionState {
    fn from(err: TcpError) -> Self {
        Self::TcpError(err)
    }
}

impl From<ConnectError> for ConnectionState {
    fn from(err: ConnectError) -> Self {
        Self::ConnectError(err)
    }
}

/// Triggers that request state evaluation or reconnection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(Format))]
pub enum ConnectionEvent {
    /// MQTT session disconnect URC (+QMTSTAT: <client_id>, <err_code>).
    /// MQTT session is lost, but cellular network/PDP context may still be active.
    MqttDisconnect(TcpError),
    /// PDP context deactivation URC (+QIURC: "pdpdeact", <context_id>).
    /// Cellular network packet data connection is deactivated.
    PdpDeactivate,
}

/// Supervisor for managing cellular & MQTT connection states, reconnection events, and backoff retries.
pub struct ConnectionSupervisor<M: AtUartTrait> {
    state: ConnectionState,
    last_connect_attempt: Option<Instant>,
    last_force_reattach: Instant,
    force_reattach_rate_limit: Duration,
    _phantom: PhantomData<M>,
}

impl<M: AtUartTrait> Default for ConnectionSupervisor<M> {
    fn default() -> Self {
        Self {
            state: ConnectionState::Disconnected,
            last_connect_attempt: None,
            last_force_reattach: Instant::now(),
            force_reattach_rate_limit: INITIAL_FORCE_REATTACH_RATE_LIMIT,
            _phantom: PhantomData,
        }
    }
}

impl<M: AtUartTrait> ConnectionSupervisor<M> {
    /// Creates a new `ConnectionSupervisor`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if currently connected to MQTT.
    pub fn is_connected(&self) -> bool {
        self.state.is_mqtt_connected()
    }

    /// Returns the current connection state.
    #[cfg(test)]
    pub fn state(&self) -> ConnectionState {
        self.state
    }

    #[cfg(test)]
    pub fn set_state(&mut self, state: ConnectionState) {
        self.state = state;
    }

    #[cfg(test)]
    pub fn set_last_connect_attempt(&mut self, now: Instant) {
        self.last_connect_attempt = Some(now);
    }

    #[cfg(test)]
    pub fn force_reattach_rate_limit(&self) -> Duration {
        self.force_reattach_rate_limit
    }

    #[cfg(test)]
    pub fn set_force_reattach_rate_limit(&mut self, limit: Duration) {
        self.force_reattach_rate_limit = limit;
    }

    /// Update the connection state and notify MQTT and cellular connection status listeners if changed
    pub fn update_status(&mut self, state: ConnectionState) {
        let mqtt_connected = state.is_mqtt_connected();
        let cellular_connected = state.is_cellular_connected();
        self.state = state;
        MQTT_CONNECTION_STATUS.sender().send_if_modified(|cur| {
            if cur.is_some_and(|v| v == mqtt_connected) {
                false
            } else {
                *cur = Some(mqtt_connected);
                true
            }
        });
        CELLULAR_CONNECTION_STATUS.sender().send_if_modified(|cur| {
            if cur.is_some_and(|v| v == cellular_connected) {
                false
            } else {
                *cur = Some(cellular_connected);
                true
            }
        });
    }

    /// Processes an incoming connection event (e.g. URC notification or publish failure).
    pub async fn handle_event(
        &mut self,
        bg77: &mut M,
        modem_manager: &mut ModemManager<M>,
        event: ConnectionEvent,
    ) {
        match event {
            ConnectionEvent::MqttDisconnect(err) => {
                warn!("MQTT disconnected ({})", err);
                if self.state == ConnectionState::MqttConnected {
                    self.update_status(err.into());
                }
            }
            ConnectionEvent::PdpDeactivate => {
                warn!("PDP deactivated");
                if modem_manager.is_registered(bg77).await != Ok(true) {
                    self.update_status(ConnectionState::CellularRegistrationFailed);
                }
            }
        }
    }

    /// Ensures that cellular attachment and MQTT connection are established.
    /// Manages 2-step registration (ModemManager -> MqttClient) and rate-limiting.
    pub async fn ensure_connected(
        &mut self,
        bg77: &mut M,
        modem_manager: &mut ModemManager<M>,
        mqtt_client: &mut MqttClient<M>,
    ) -> crate::Result<()> {
        self.ensure_connected_at(bg77, modem_manager, mqtt_client, Instant::now()).await
    }

    /// Ensures that cellular attachment and MQTT connection are established at the given instant.
    pub async fn ensure_connected_at(
        &mut self,
        bg77: &mut M,
        modem_manager: &mut ModemManager<M>,
        mqtt_client: &mut MqttClient<M>,
        now: Instant,
    ) -> crate::Result<()> {
        // Short-circuit if already connected
        if self.state == ConnectionState::MqttConnected {
            return Ok(());
        }

        // Rate-limiting check for connection attempts
        if let Some(last_attempt) = self.last_connect_attempt
            && last_attempt + mqtt_client.packet_timeout() * 2 > now
        {
            return Ok(());
        }
        self.last_connect_attempt = Some(now);

        let force_reattach = mqtt_client.last_successful_send() + MAX_INACTIVE_FORCE_REATTACH < now
            && self.last_force_reattach + self.force_reattach_rate_limit <= now;
        // Step 1: Cellular Registration
        // Short-circuit if cellular attachment already succeeded and no force reattach is required
        if !self.state.is_mqtt_error() || force_reattach {
            if force_reattach {
                self.last_force_reattach = now;
                self.force_reattach_rate_limit =
                    (self.force_reattach_rate_limit * 2).min(MAX_FORCE_REATTACH_RATE_LIMIT);
            }
            self.update_status(ConnectionState::ConnectingCellular);
            if let Err(err) = modem_manager.network_registration(bg77, force_reattach).await {
                error!("Cellular network registration failed: {}", err);
                self.update_status(ConnectionState::CellularRegistrationFailed);
                return Err(err.into());
            }
        }

        self.ensure_mqtt_connected(bg77, mqtt_client).await
    }

    /// Ensures that the MQTT client is connected.
    pub async fn ensure_mqtt_connected(
        &mut self,
        bg77: &mut M,
        mqtt_client: &mut MqttClient<M>,
    ) -> crate::Result<()> {
        self.update_status(ConnectionState::ConnectingMqtt);
        if let Err(err) = mqtt_client.open(bg77).await {
            error!("MQTT open failed: {}", err);
            self.update_status(err.into());
            return Err(err.into());
        }

        match mqtt_client.connect(bg77).await {
            Ok(()) => {
                self.on_connection_success();
                Ok(())
            }
            Err(ConnectError::InProgress) => {
                self.update_status(ConnectionState::ConnectingMqtt);
                Ok(())
            }
            Err(err) => {
                error!("MQTT connection failed: {}", err);
                self.update_status(err.into());
                Err(err.into())
            }
        }
    }

    /// Check whether the stored state matches the modem state
    pub async fn check_state(
        &mut self,
        bg77: &mut M,
        modem_manager: &mut ModemManager<M>,
        mqtt_client: &mut MqttClient<M>,
    ) {
        info!("Checking modem connection status");
        if modem_manager.is_registered(bg77).await != Ok(true) {
            self.update_status(ConnectionState::CellularRegistrationFailed);
        } else if mqtt_client.is_connected(bg77).await != Ok(true) {
            if !self.state.is_mqtt_error() {
                self.update_status(TcpError::ServerDisconnect.into());
            }
        } else {
            self.on_connection_success();
        }
    }

    fn on_connection_success(&mut self) {
        self.update_status(ConnectionState::MqttConnected);
        self.force_reattach_rate_limit = INITIAL_FORCE_REATTACH_RATE_LIMIT;
    }
}

#[cfg(feature = "std")]
#[cfg(test)]
mod tests {
    use super::*;
    use embassy_futures::block_on;

    use crate::at::fake_modem::FakeModem;
    use crate::bg77::modem_manager::ModemConfig;
    use crate::mqtt::MqttClientConfig;

    #[test]
    fn test_connection_state_default() {
        let state = ConnectionState::default();
        assert_eq!(state, ConnectionState::Disconnected);
        assert!(!state.is_mqtt_connected());
        assert!(!state.is_connecting());
    }

    #[test]
    fn test_connection_state_queries() {
        assert!(ConnectionState::MqttConnected.is_mqtt_connected());
        assert!(!ConnectionState::MqttConnected.is_connecting());

        assert!(!ConnectionState::ConnectingCellular.is_mqtt_connected());
        assert!(ConnectionState::ConnectingCellular.is_connecting());

        assert!(!ConnectionState::ConnectingMqtt.is_mqtt_connected());
        assert!(ConnectionState::ConnectingMqtt.is_connecting());
    }

    #[test]
    fn test_supervisor_handle_events() {
        let mut supervisor = ConnectionSupervisor::<FakeModem>::new();
        let mut modem_manager = ModemManager::<FakeModem>::new(ModemConfig::default());
        assert_eq!(supervisor.state(), ConnectionState::Disconnected);

        supervisor.state = ConnectionState::MqttConnected;
        let mut bg77 = FakeModem::new(&[]);
        block_on(supervisor.handle_event(
            &mut bg77,
            &mut modem_manager,
            ConnectionEvent::MqttDisconnect(TcpError::NetworkDisconnected),
        ));
        assert_eq!(
            supervisor.state(),
            ConnectionState::TcpError(TcpError::NetworkDisconnected)
        );

        // When registered, PdpDeactivate is ignored (e.g. stale URC)
        let mut bg77_registered =
            FakeModem::new(&[("AT+CGATT?", "+CGATT: 1"), ("AT+CGACT?", "+CGACT: 1,1")]);
        block_on(supervisor.handle_event(
            &mut bg77_registered,
            &mut modem_manager,
            ConnectionEvent::PdpDeactivate,
        ));
        assert_eq!(
            supervisor.state(),
            ConnectionState::TcpError(TcpError::NetworkDisconnected)
        );

        // When not registered, PdpDeactivate transitions to CellularRegistrationFailed
        let mut bg77_unregistered = FakeModem::new(&[("AT+CGATT?", "+CGATT: 0")]);
        block_on(supervisor.handle_event(
            &mut bg77_unregistered,
            &mut modem_manager,
            ConnectionEvent::PdpDeactivate,
        ));
        assert_eq!(
            supervisor.state(),
            ConnectionState::CellularRegistrationFailed
        );

        // MqttDisconnect must not overwrite CellularRegistrationFailed
        let mut bg77 = FakeModem::new(&[]);
        block_on(supervisor.handle_event(
            &mut bg77,
            &mut modem_manager,
            ConnectionEvent::MqttDisconnect(TcpError::NetworkDisconnected),
        ));
        assert_eq!(
            supervisor.state(),
            ConnectionState::CellularRegistrationFailed
        );
    }

    #[test]
    fn test_supervisor_ensure_connected_ok() {
        let mut supervisor = ConnectionSupervisor::<FakeModem>::new();
        let mut modem_manager = ModemManager::<FakeModem>::new(ModemConfig::default());
        let mut mqtt_client = MqttClient::<FakeModem>::new(MqttClientConfig::default(), 1);

        let mut bg77 = FakeModem::new(&[
            ("AT+CGATT?", "+CGATT: 1"),
            ("AT+CGACT?", "+CGACT: 1,1"),
            ("AT+QMTOPEN?", ""),
            ("AT+QMTCFG=\"timeout\",1,35,2,1", ""),
            ("AT+QMTCFG=\"keepalive\",1,70", ""),
            (
                "AT+QMTCFG=\"will\",1,1,1,0,\"yar/deadbeef/will\",\"test_client\"",
                "",
            ),
            ("AT+QMTOPEN=1,\"broker.emqx.io\",1883", "+QMTOPEN: 1,0"),
            ("AT+QMTCONN?", "+QMTCONN: 1,1"),
            ("AT+QMTCONN=1,\"test_client\"", "+QMTCONN: 1,0,0"),
        ]);

        let res =
            block_on(supervisor.ensure_connected(&mut bg77, &mut modem_manager, &mut mqtt_client));

        assert!(res.is_ok());
        assert_eq!(supervisor.state(), ConnectionState::MqttConnected);
    }

    #[test]
    fn test_supervisor_force_reattach_rate_limiting() {
        let mut supervisor = ConnectionSupervisor::<FakeModem>::new();
        let mut modem_manager = ModemManager::<FakeModem>::new(ModemConfig::default());
        let mut mqtt_client = MqttClient::<FakeModem>::new(MqttClientConfig::default(), 1);

        supervisor.state = ConnectionState::TcpError(TcpError::FailedToOpenNetwork);
        supervisor.last_connect_attempt = None;

        // Since state is TcpError and last_force_reattach is recent (within 210s),
        // force_reattach evaluates to false, skipping cellular reattachment.
        let mut bg77 = FakeModem::new(&[
            ("AT+QMTOPEN?", ""),
            ("AT+QMTCFG=\"timeout\",1,35,2,1", ""),
            ("AT+QMTCFG=\"keepalive\",1,70", ""),
            (
                "AT+QMTCFG=\"will\",1,1,1,0,\"yar/deadbeef/will\",\"test_client\"",
                "",
            ),
            ("AT+QMTOPEN=1,\"broker.emqx.io\",1883", "+QMTOPEN: 1,0"),
            ("AT+QMTCONN?", "+QMTCONN: 1,1"),
            ("AT+QMTCONN=1,\"test_client\"", "+QMTCONN: 1,0,0"),
        ]);

        let res =
            block_on(supervisor.ensure_connected(&mut bg77, &mut modem_manager, &mut mqtt_client));
        assert!(res.is_ok());
        assert_eq!(supervisor.state(), ConnectionState::MqttConnected);
    }

    #[test]
    fn test_supervisor_force_reattach_exponential_backoff() {
        let start = Instant::now();
        let mut supervisor = ConnectionSupervisor::<FakeModem>::new();
        let mut modem_manager = ModemManager::<FakeModem>::new(ModemConfig::default());
        let mut mqtt_client = MqttClient::<FakeModem>::new(MqttClientConfig::default(), 1);

        assert_eq!(
            supervisor.force_reattach_rate_limit(),
            Duration::from_secs(210)
        );

        supervisor.set_state(ConnectionState::TcpError(TcpError::NetworkDisconnected));

        const REATTACH_RESPONSES: &[(&str, &str)] = &[
            ("ATE0", ""),
            ("AT+CGATT=0", ""),
            ("AT+CGACT=0,1", ""),
            ("AT+CGATT=1", ""),
            ("AT+CGACT?", "+CGACT: 1,1"),
            ("AT+QMTOPEN?", ""),
            ("AT+QMTCFG=\"timeout\",1,35,2,1", ""),
            ("AT+QMTCFG=\"keepalive\",1,70", ""),
            (
                "AT+QMTCFG=\"will\",1,1,1,0,\"yar/deadbeef/will\",\"test_client\"",
                "",
            ),
            ("AT+QMTOPEN=1,\"broker.emqx.io\",1883", "+QMTOPEN: 1,-1"),
        ];

        // 1st force reattach (at 211s): rate limit doubles from 210s to 420s
        let mut bg77 = FakeModem::new(REATTACH_RESPONSES);
        let t1 = start + Duration::from_secs(211);
        let _ = block_on(supervisor.ensure_connected_at(
            &mut bg77,
            &mut modem_manager,
            &mut mqtt_client,
            t1,
        ));
        assert_eq!(
            supervisor.force_reattach_rate_limit(),
            Duration::from_secs(420)
        );

        // 2nd force reattach (after 421s): doubles to 840s
        let mut bg77 = FakeModem::new(REATTACH_RESPONSES);
        let t2 = t1 + Duration::from_secs(421);
        let _ = block_on(supervisor.ensure_connected_at(
            &mut bg77,
            &mut modem_manager,
            &mut mqtt_client,
            t2,
        ));
        assert_eq!(
            supervisor.force_reattach_rate_limit(),
            Duration::from_secs(840)
        );

        // Test capping: set rate limit near max and verify it is capped at 45 minutes
        supervisor.set_force_reattach_rate_limit(Duration::from_secs(30 * 60));
        let mut bg77 = FakeModem::new(REATTACH_RESPONSES);
        let t3 = t2 + Duration::from_secs(30 * 60 + 1);
        let _ = block_on(supervisor.ensure_connected_at(
            &mut bg77,
            &mut modem_manager,
            &mut mqtt_client,
            t3,
        ));
        assert_eq!(
            supervisor.force_reattach_rate_limit(),
            Duration::from_secs(45 * 60)
        );

        // Connection success resets back to initial 210s
        supervisor.on_connection_success();
        assert_eq!(
            supervisor.force_reattach_rate_limit(),
            Duration::from_secs(210)
        );
    }

    #[test]
    fn test_supervisor_reconnect_rate_limiting() {
        let mut supervisor = ConnectionSupervisor::<FakeModem>::new();
        let mut modem_manager = ModemManager::<FakeModem>::new(ModemConfig::default());
        let mut mqtt_client = MqttClient::<FakeModem>::new(MqttClientConfig::default(), 1);

        supervisor.set_last_connect_attempt(Instant::now());
        let mut bg77 = FakeModem::new(&[]);

        let res =
            block_on(supervisor.ensure_connected(&mut bg77, &mut modem_manager, &mut mqtt_client));
        assert!(res.is_ok());
        assert_eq!(supervisor.state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_update_status() {
        let mut supervisor = ConnectionSupervisor::<FakeModem>::new();
        supervisor.update_status(ConnectionState::ConnectingMqtt);
        let mut mqtt_rx = MQTT_CONNECTION_STATUS.receiver().unwrap();
        let mut cellular_rx = CELLULAR_CONNECTION_STATUS.receiver().unwrap();
        assert_eq!(mqtt_rx.try_get(), Some(false));
        assert_eq!(cellular_rx.try_get(), Some(true));

        supervisor.update_status(ConnectionState::MqttConnected);
        assert_eq!(mqtt_rx.try_get(), Some(true));
        assert_eq!(cellular_rx.try_get(), Some(true));
        assert_eq!(supervisor.state(), ConnectionState::MqttConnected);

        supervisor.update_status(ConnectionState::CellularRegistrationFailed);
        assert_eq!(mqtt_rx.try_get(), Some(false));
        assert_eq!(cellular_rx.try_get(), Some(false));
    }

    #[test]
    fn test_supervisor_ensure_connected_in_progress() {
        let mut supervisor = ConnectionSupervisor::<FakeModem>::new();
        let mut modem_manager = ModemManager::<FakeModem>::new(ModemConfig::default());
        let mut mqtt_client = MqttClient::<FakeModem>::new(MqttClientConfig::default(), 1);

        let mut bg77 = FakeModem::new(&[
            ("AT+CGATT?", "+CGATT: 1"),
            ("AT+CGACT?", "+CGACT: 1,1"),
            ("AT+QMTOPEN?", ""),
            ("AT+QMTCFG=\"timeout\",1,35,2,1", ""),
            ("AT+QMTCFG=\"keepalive\",1,70", ""),
            (
                "AT+QMTCFG=\"will\",1,1,1,0,\"yar/deadbeef/will\",\"test_client\"",
                "",
            ),
            ("AT+QMTOPEN=1,\"broker.emqx.io\",1883", "+QMTOPEN: 1,0"),
            ("AT+QMTCONN?", "+QMTCONN: 1,2"),
        ]);

        let res =
            block_on(supervisor.ensure_connected(&mut bg77, &mut modem_manager, &mut mqtt_client));
        assert!(res.is_ok());
        assert_eq!(supervisor.state(), ConnectionState::ConnectingMqtt);
        assert!(!supervisor.is_connected());
        assert!(supervisor.state().is_connecting());
    }
}
