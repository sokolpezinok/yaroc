use embassy_time::Duration;

#[cfg(feature = "defmt")]
use defmt::Format;

/// Explicit state of the cellular and MQTT connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(Format))]
pub enum ConnectionState {
    /// Completely disconnected from cellular network and MQTT broker.
    #[default]
    Disconnected,
    /// Registering to cellular network (AT+CGATT / AT+CGACT).
    ConnectingCellular,
    /// Opening TCP socket and establishing MQTT session (+QIOPEN / +QMTCONN).
    ConnectingMqtt,
    /// Fully connected to MQTT broker and ready to transmit payloads.
    Connected,
    /// In backoff wait period before next reconnection attempt.
    BackoffWait(Duration),
}

impl ConnectionState {
    /// Returns `true` if currently connected to MQTT.
    #[inline]
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected)
    }

    /// Returns `true` if currently in any connecting phase.
    #[inline]
    pub fn is_connecting(&self) -> bool {
        matches!(self, Self::ConnectingCellular | Self::ConnectingMqtt)
    }
}

/// Triggers that request state evaluation or reconnection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(Format))]
pub enum ConnectionEvent {
    /// MQTT session disconnect URC (+QMTSTAT: <client_id>, <err_code>).
    /// MQTT session is lost, but cellular network/PDP context may still be active.
    MqttDisconnect(u8),
    /// PDP context deactivation URC (+QIURC: "pdpdeact", <context_id>).
    /// Cellular network packet data connection is deactivated.
    PdpDeactivate(u8),
    /// MQTT publish failure.
    PublishFailed,
    /// Periodic status/keepalive check tick.
    PeriodicCheck,
    /// Forced manual reconnection request.
    ForceReconnect,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_state_default() {
        let state = ConnectionState::default();
        assert_eq!(state, ConnectionState::Disconnected);
        assert!(!state.is_connected());
        assert!(!state.is_connecting());
    }

    #[test]
    fn test_connection_state_queries() {
        assert!(ConnectionState::Connected.is_connected());
        assert!(!ConnectionState::Connected.is_connecting());

        assert!(!ConnectionState::ConnectingCellular.is_connected());
        assert!(ConnectionState::ConnectingCellular.is_connecting());

        assert!(!ConnectionState::ConnectingMqtt.is_connected());
        assert!(ConnectionState::ConnectingMqtt.is_connecting());
    }
}
