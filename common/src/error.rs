use defmt::Format;
use thiserror::Error;

#[derive(Debug, Error, Eq, Format, PartialEq)]
pub enum Error {
    #[error("Buffer too small")]
    BufferTooSmallError,
    #[error("Cannot parse string as the given type")]
    ParseError,
    #[error("Inconsistent AT response")]
    AtError,
}
