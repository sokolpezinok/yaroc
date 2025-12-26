use thiserror::Error;

#[derive(Debug, Error, Eq, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error {
    #[error("Buffer too small")]
    BufferTooSmallError,
    #[error("Formatting error, usually buffer too small")]
    FormatError,
    #[error("Cannot parse string as the given type")]
    ParseError,
    #[error("Postcard parsing error")]
    PostcardParseError(#[from] postcard::Error),
    #[error("Supplied wrong function argument")]
    ValueError,
    #[error("Softdevice (BLE) error")]
    SoftdeviceError,
    #[error("Flash (NVM) error")]
    FlashError,
    #[error("Inconsistent AT response")]
    ModemError,
    #[error("UART read error")]
    UartReadError,
    #[cfg(feature = "nrf")]
    #[error("UART write error: {0:?}")]
    UartWriteError(embassy_nrf::uarte::Error),
    #[error("USB read error")]
    UsbReadError,
    #[error("USB disconnected")]
    UsbDisconnected,
    #[error("USB write error")]
    UsbWriteError,
    #[error("UART unexpectedly closed")]
    UartClosedError,
    #[error("AT 'ERROR' response")]
    AtErrorResponse,
    #[error("Timeout error")]
    TimeoutError,
    #[error("String encoding error")]
    StringEncodingError,
    #[error("Network registrarion error")]
    NetworkRegistrationError,
    #[error("MQTT error {0}")]
    MqttError(i8),
    #[error("Semaphore synchronization error")]
    SemaphoreError,
}

impl embedded_io_async::Error for Error {
    fn kind(&self) -> embedded_io_async::ErrorKind {
        match self {
            Error::TimeoutError => embedded_io_async::ErrorKind::TimedOut,
            _ => embedded_io_async::ErrorKind::Other,
        }
    }
}

impl From<core::fmt::Error> for Error {
    fn from(_: core::fmt::Error) -> Self {
        Error::FormatError
    }
}

#[cfg(feature = "nrf")]
impl From<embassy_nrf::uarte::Error> for Error {
    fn from(e: embassy_nrf::uarte::Error) -> Self {
        Error::UartWriteError(e)
    }
}
