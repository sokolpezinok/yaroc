use thiserror::Error;

use crate::at::AtError;
use crate::bg77::modem_manager::RegistrationError;
use crate::bg77::mqtt::{ConnectError, PublishError, TcpError};

#[derive(Debug, Error, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error {
    #[error("Buffer too small")]
    BufferTooSmallError,
    #[error("Formatting error, usually buffer too small")]
    FormatError,
    #[error("Cannot parse string as the given type")]
    ParseError,
    #[error("Protobuf parse error: {0}")]
    FemtopbDecodeError(femtopb::error::DecodeError),
    #[error("Postcard parsing error")]
    PostcardParseError(#[from] postcard::Error),
    #[error("Supplied wrong function argument")]
    ValueError,
    #[error("Softdevice (BLE) error")]
    SoftdeviceError,
    #[error("Flash (NVM) error")]
    FlashError,
    #[error("UART read error")]
    UartReadError,
    #[error("USB read error")]
    UsbReadError,
    #[error("USB disconnected")]
    UsbDisconnected,
    #[error("USB write error")]
    UsbWriteError,
    #[error("UART unexpectedly closed")]
    UartClosedError,
    #[error(transparent)]
    At(#[from] AtError),
    #[error("Network registration error: {0}")]
    Registration(#[from] RegistrationError),
    #[error("MQTT TCP error: {0}")]
    MqttTcp(#[from] TcpError),
    #[error("MQTT connect error: {0}")]
    MqttConnect(#[from] ConnectError),
    #[error("MQTT publish error: {0}")]
    MqttPublish(#[from] PublishError),
    #[error("Semaphore synchronization error")]
    SemaphoreError,
    #[error("Timeout error")]
    TimeoutError,
    #[error("Not connected")]
    NotConnected,
}

impl From<core::fmt::Error> for Error {
    fn from(_: core::fmt::Error) -> Self {
        Error::FormatError
    }
}

impl From<core::convert::Infallible> for Error {
    fn from(_: core::convert::Infallible) -> Self {
        unreachable!()
    }
}

impl From<femtopb::error::DecodeError> for Error {
    fn from(e: femtopb::error::DecodeError) -> Self {
        Error::FemtopbDecodeError(e)
    }
}
