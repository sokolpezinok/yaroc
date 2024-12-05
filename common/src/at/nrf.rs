use embassy_executor::Spawner;
use embassy_nrf::{
    peripherals::{TIMER0, UARTE1},
    uarte::{UarteRxWithIdle, UarteTx},
};

use super::uart::{reader_task, RxWithIdle, Tx, MAIN_RX_CHANNEL};

impl Tx for UarteTx<'static, UARTE1> {
    async fn write(&mut self, buffer: &[u8]) -> crate::Result<()> {
        self.write(buffer).await.map_err(|_| crate::error::Error::UartWriteError)
    }
}

/// RX reader task implemented for UarteRxWithIdle.
#[embassy_executor::task]
async fn reader(
    rx: UarteRxWithIdle<'static, UARTE1, TIMER0>,
    urc_classifier: fn(&str, &str) -> bool,
) {
    reader_task(rx, urc_classifier, &MAIN_RX_CHANNEL).await;
}

impl RxWithIdle for UarteRxWithIdle<'static, UARTE1, TIMER0> {
    fn spawn(self, spawner: &Spawner, urc_classifier: fn(&str, &str) -> bool) {
        spawner.must_spawn(reader(self, urc_classifier));
    }

    async fn read_until_idle(&mut self, buf: &mut [u8]) -> crate::Result<usize> {
        self.read_until_idle(buf).await.map_err(|_| crate::error::Error::UartReadError)
    }
}
