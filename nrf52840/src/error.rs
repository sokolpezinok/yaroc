use defmt::Format;
use thiserror::Error;

#[derive(Debug, Error, Format)]
pub enum Error {
    #[error("Buffer too small")]
    BufferTooSmallError,
    #[error("String encoding error")]
    StringEncodingError,
    #[error("UART read error")]
    UartReadError,
    #[error("UART write error")]
    UartWriteError,
    #[error("Timeout error")]
    TimeoutError,
    #[error("AT error")]
    AtError,
}
