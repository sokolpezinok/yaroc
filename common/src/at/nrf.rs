use crate::error::Error;
use embassy_executor::Spawner;
use embassy_nrf::{
    peripherals::{TIMER0, UARTE1},
    uarte::{UarteRxWithIdle, UarteTx},
};

use super::uart::{AtRxBroker, RxWithIdle, Tx, UrcHandlerType, MAIN_RX_CHANNEL};

impl Tx for UarteTx<'static, UARTE1> {
    async fn write(&mut self, buffer: &[u8]) -> crate::Result<()> {
        self.write(buffer).await.map_err(|_| Error::UartWriteError)
    }
}

/// RX broker loop implemented for UarteRxWithIdle.
#[embassy_executor::task]
async fn reader(rx: UarteRxWithIdle<'static, UARTE1, TIMER0>, at_broker: AtRxBroker) {
    at_broker.broker_loop(rx).await;
}

impl RxWithIdle for UarteRxWithIdle<'static, UARTE1, TIMER0> {
    fn spawn(self, spawner: Spawner, urc_handler: UrcHandlerType) {
        let at_broker = AtRxBroker::new(&MAIN_RX_CHANNEL, urc_handler);
        spawner.must_spawn(reader(self, at_broker));
    }

    async fn read_until_idle(&mut self, buf: &mut [u8]) -> crate::Result<usize> {
        self.read_until_idle(buf).await.map_err(|_| Error::UartReadError)
    }
}
