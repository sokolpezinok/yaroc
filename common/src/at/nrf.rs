use embassy_nrf::{peripherals::UARTE1, uarte::UarteTx};

use super::uart::Tx;

impl Tx for UarteTx<'static, UARTE1> {
    async fn write(&mut self, buffer: &[u8]) -> crate::Result<()> {
        self.write(buffer).await.map_err(|_| crate::error::Error::UartWriteError)
    }
}
