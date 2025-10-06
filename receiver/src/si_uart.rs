use tokio::io::AsyncReadExt;
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use yaroc_common::{error::Error, si_uart::RxWithIdle};

pub struct TokioSerial {
    serial: SerialStream,
}

impl TokioSerial {
    pub fn new(port: &str) -> crate::Result<Self> {
        let builder = tokio_serial::new(port, 38400);
        let serial =
            builder.open_native_async().map_err(|_| crate::error::Error::ConnectionError)?;
        Ok(Self { serial })
    }
}

impl RxWithIdle for TokioSerial {
    async fn read_until_idle(&mut self, buf: &mut [u8]) -> yaroc_common::Result<usize> {
        self.serial.read(buf).await.map_err(|_| Error::UartReadError)
    }
}
