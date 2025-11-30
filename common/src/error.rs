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
    #[error("UART write error")]
    UartWriteError,
    #[error("USB read error")]
    UsbReadError,
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

impl From<core::fmt::Error> for Error {
    fn from(_: core::fmt::Error) -> Self {
        Error::FormatError
    }
}
