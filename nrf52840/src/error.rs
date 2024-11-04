#[derive(Debug)]
pub enum Error {
    BufferTooSmallError,
    StringEncodingError,
    UartReadError,
    UartWriteError,
}
