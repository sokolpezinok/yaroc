use common::at::uart::{AtBroker, MainChannelType, RxWithIdle, Tx};
use core::str::from_utf8;
use defmt::*;
use embassy_executor::Spawner;
use embassy_nrf::peripherals::{TIMER0, UARTE1};
use embassy_nrf::uarte::{UarteRxWithIdle as EmbassyUarteRxWithIdle, UarteTx as EmbassyUarteTx};
use embassy_sync::channel::Channel;

pub static URC_CHANNEL: common::at::uart::UrcChannelType = Channel::new();

/// RX reader task implemented for UarteRxWithIdle.
#[embassy_executor::task]
async fn reader(
    mut rx: EmbassyUarteRxWithIdle<'static, UARTE1, TIMER0>,
    urc_classifier: fn(&str, &str) -> bool,
    main_channel: &'static MainChannelType<common::error::Error>,
) {
    const AT_BUF_SIZE: usize = 300;
    let mut buf = [0; AT_BUF_SIZE];
    let at_broker = AtBroker::new(main_channel, &URC_CHANNEL);
    loop {
        let len = rx
            .read_until_idle(&mut buf)
            .await
            .map_err(|_| common::error::Error::UartReadError);
        match len {
            Err(err) => main_channel.send(Err(err)).await,
            Ok(len) => {
                let text = from_utf8(&buf[..len]);
                match text {
                    Err(_) => {
                        main_channel.send(Err(common::error::Error::StringEncodingError)).await
                    }
                    Ok(text) => at_broker.parse_lines(text, urc_classifier).await,
                }
            }
        }
    }
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
    fn spawn(
        self,
        spawner: &Spawner,
        urc_classifier: fn(&str, &str) -> bool,
        main_channel: &'static MainChannelType<common::error::Error>,
    ) {
        unwrap!(spawner.spawn(reader(self.rx, urc_classifier, main_channel)));
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
