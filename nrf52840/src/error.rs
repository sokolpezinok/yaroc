use defmt::Format;
use thiserror::Error;

#[derive(Debug, Error, Format)]
pub enum Error {
    #[error("Buffer too small")]
    BufferTooSmallError,
    #[error("Cannot parse string as the given type")]
    ParseError,
    #[error("String encoding error")]
    StringEncodingError,
    #[error("UART read error")]
    UartReadError,
    #[error("UART write error")]
    UartWriteError,
    #[error("Timeout error")]
    TimeoutError,
    #[error("AT 'ERROR' response")]
    AtErrorResponse,
    #[error("AT error")]
    AtError,
    #[error("MQTT error {0}")]
    MqttError(i8)
}
