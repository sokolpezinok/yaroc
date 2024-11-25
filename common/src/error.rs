use thiserror::Error;

#[derive(Debug, Error, Eq, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error {
    #[error("Buffer too small")]
    BufferTooSmallError,
    #[error("Cannot parse string as the given type")]
    ParseError,
    #[error("Inconsistent AT response")]
    ModemError,
}
