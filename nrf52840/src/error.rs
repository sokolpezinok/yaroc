use common::error as common_error;
use defmt::Format;
use thiserror::Error;

#[derive(Debug, Error, Format)]
pub enum Error {
    #[error("Buffer too small")]
    BufferTooSmallError,
    #[error("Formatting error")]
    FormatError,
    #[error("Cannot parse string as the given type")]
    ParseError,
    #[error("String encoding error")]
    StringEncodingError,
    #[error("UART read error")]
    UartReadError,
    #[error("UART write error")]
    UartWriteError,
    #[error("AT 'ERROR' response")]
    AtErrorResponse,
    #[error("Timeout error")]
    TimeoutError,
    #[error("Unexpected response from the modem")]
    ModemError,
    #[error("Network registrarion error")]
    NetworkRegistrationError,
    #[error("MQTT error {0}")]
    MqttError(i8),
}

impl From<common_error::Error> for Error {
    fn from(err: common_error::Error) -> Self {
        match err {
            common_error::Error::BufferTooSmallError => Self::BufferTooSmallError,
            common_error::Error::ModemError => Self::ModemError,
            common_error::Error::ParseError => Self::ParseError,
            common_error::Error::UartReadError => Self::UartReadError,
            common_error::Error::UartWriteError => Self::UartWriteError,
            common_error::Error::StringEncodingError => Self::StringEncodingError,
        }
    }
}

impl From<core::fmt::Error> for Error {
    fn from(_: core::fmt::Error) -> Self {
        Error::FormatError
    }
}
