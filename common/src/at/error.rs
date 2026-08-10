use thiserror::Error;

#[derive(Debug, Error, Eq, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum AtError {
    #[error("Buffer too small")]
    BufferTooSmallError,
    #[error("Cannot parse string as the given type")]
    ParseError,
    #[error("Inconsistent AT response")]
    ModemError,
    #[error("AT 'ERROR' response")]
    AtErrorResponse,
    #[error("CME error: {0}")]
    CmeError(u16),
    #[error("Timeout error")]
    TimeoutError,
    #[error("UART read error")]
    UartReadError,
    #[cfg(feature = "nrf")]
    #[error("UART write error: {0:?}")]
    UartWriteError(embassy_nrf::uarte::Error),
}

#[cfg(any(test, feature = "std"))]
impl From<core::convert::Infallible> for AtError {
    fn from(_: core::convert::Infallible) -> Self {
        unreachable!()
    }
}

#[cfg(feature = "nrf")]
impl From<embassy_nrf::uarte::Error> for AtError {
    fn from(e: embassy_nrf::uarte::Error) -> Self {
        AtError::UartWriteError(e)
    }
}
