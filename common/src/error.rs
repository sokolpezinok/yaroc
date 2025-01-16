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
    #[error("Inconsistent AT response")]
    ModemError,
    #[error("UART read error")]
    UartReadError,
    #[error("UART write error")]
    UartWriteError,
    #[error("AT 'ERROR' response")]
    AtErrorResponse,
    #[error("Timeout error")]
    TimeoutError,
    #[error("String encoding error")]
    StringEncodingError,
}

impl From<core::fmt::Error> for Error {
    fn from(_: core::fmt::Error) -> Self {
        Error::FormatError
    }
}
