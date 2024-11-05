use defmt::Format;

#[derive(Debug, Format)]
pub enum Error {
    BufferTooSmallError,
    StringEncodingError,
    UartReadError,
    UartWriteError,
    TimeoutError,
}
