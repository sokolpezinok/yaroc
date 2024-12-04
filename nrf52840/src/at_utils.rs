use common::at::uart::{reader_task, RxWithIdle, Tx, MAIN_RX_CHANNEL};
use defmt::*;
use embassy_executor::Spawner;
use embassy_nrf::peripherals::{TIMER0, UARTE1};
use embassy_nrf::uarte::{UarteRxWithIdle as EmbassyUarteRxWithIdle, UarteTx as EmbassyUarteTx};

/// RX reader task implemented for UarteRxWithIdle.
#[embassy_executor::task]
async fn reader(rx: UarteRxWithIdle, urc_classifier: fn(&str, &str) -> bool) {
    reader_task(rx, urc_classifier, &MAIN_RX_CHANNEL).await;
}

pub struct UarteRxWithIdle {
    // This struct is fixed to UARTE1 due to a limitation of embassy_executor::task. We cannot make
    // the `reader` method generic and also work for UARTE0. However, for our hardware this is not
    // needed, UARTE0 does not use AT-commands, so it won't use this struct.
    rx: EmbassyUarteRxWithIdle<'static, UARTE1, TIMER0>,
}

impl UarteRxWithIdle {
    pub fn new(rx: EmbassyUarteRxWithIdle<'static, UARTE1, TIMER0>) -> Self {
        Self { rx }
    }
}

impl RxWithIdle for UarteRxWithIdle {
    fn spawn(self, spawner: &Spawner, urc_classifier: fn(&str, &str) -> bool) {
        unwrap!(spawner.spawn(reader(self, urc_classifier)));
    }

    async fn read_until_idle(&mut self, buf: &mut [u8]) -> common::Result<usize> {
        self.rx
            .read_until_idle(buf)
            .await
            .map_err(|_| common::error::Error::UartReadError)
    }
}

pub struct UarteTx {
    // This struct is fixed to UARTE1 due to a limitation of embassy_executor::task. We cannot make
    // the `reader` method generic and also work for UARTE0. However, for our hardware this is not
    // needed, UARTE0 does not use AT-commands, so it won't use this struct.
    tx: EmbassyUarteTx<'static, UARTE1>,
}

impl UarteTx {
    pub fn new(tx: EmbassyUarteTx<'static, UARTE1>) -> Self {
        Self { tx }
    }
}

impl Tx for UarteTx {
    async fn write(&mut self, buffer: &[u8]) -> common::Result<()> {
        self.tx.write(buffer).await.map_err(|_| common::error::Error::UartWriteError)
    }
}
