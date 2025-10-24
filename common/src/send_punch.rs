use embassy_time::Instant;

/// Commands to be sent to the `send_punch_event_handler`.
pub enum SendPunchCommand {
    /// Instructs the modem to synchronize its time with the network.
    SynchronizeTime,
    /// Instructs the modem to connect to the MQTT broker.
    ///
    /// The `bool` parameter indicates whether to force a reconnection.
    MqttConnect(bool, Instant),
    /// Instructs the modem to update the battery status.
    BatteryUpdate,
}
