use crate::error::Error;
use embassy_executor::Spawner;
use embassy_nrf::uarte::UarteRxWithIdle;

use super::uart::{AtRxBroker, MAIN_RX_CHANNEL, RxWithIdle, UrcHandlerType};

/// RX broker loop implemented for UarteRxWithIdle.
#[embassy_executor::task]
async fn reader(rx: UarteRxWithIdle<'static>, at_broker: AtRxBroker) {
    at_broker.broker_loop(rx).await;
}

impl RxWithIdle for UarteRxWithIdle<'static> {
    fn spawn(self, spawner: Spawner, urc_handlers: &[UrcHandlerType]) {
        let at_broker = AtRxBroker::new(&MAIN_RX_CHANNEL, urc_handlers);
        spawner.spawn(reader(self, at_broker).expect("Failed to spawn task"));
    }

    async fn read_until_idle(&mut self, buf: &mut [u8]) -> crate::Result<usize> {
        self.read_until_idle(buf).await.map_err(|_| Error::UartReadError)
    }
}
