use common::at::response::{FromModem, AT_COMMAND_SIZE};
use common::at::uart::{AtRxBroker, RxWithIdle, Tx, MAIN_RX_CHANNEL};
use common::{at::uart::AtUart, error::Error};
use embassy_executor::{Executor, Spawner};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::Duration;
use heapless::String;
use static_cell::StaticCell;
use std::str::FromStr;

struct FakeRxWithIdle {
    responses: Vec<(&'static str, &'static str)>,
}

impl FakeRxWithIdle {
    fn new(responses: Vec<(&'static str, &'static str)>) -> Self {
        Self { responses }
    }
}

type TxChannelType = Channel<CriticalSectionRawMutex, String<AT_COMMAND_SIZE>, 5>;
static TX_CHANNEL: TxChannelType = Channel::new();

#[embassy_executor::task]
async fn reader(rx: FakeRxWithIdle, urc_classifier: fn(&str, &str) -> bool) {
    AtRxBroker::broker_loop(rx, urc_classifier, &MAIN_RX_CHANNEL).await;
}

impl RxWithIdle for FakeRxWithIdle {
    fn spawn(self, spawner: &Spawner, urc_classifier: fn(&str, &str) -> bool) {
        spawner.must_spawn(reader(self, urc_classifier))
    }

    async fn read_until_idle(&mut self, buf: &mut [u8]) -> common::Result<usize> {
        let recv_command = TX_CHANNEL.receive().await;
        if let Some((command, response)) = self.responses.first() {
            assert_eq!(command, &recv_command);
            let bytes = response.as_bytes();
            buf[..bytes.len()].clone_from_slice(bytes);
            self.responses.remove(0);
            Ok(bytes.len())
        } else {
            Err(common::error::Error::TimeoutError)
        }
    }
}

struct FakeTx {
    channel: &'static TxChannelType,
}

impl FakeTx {
    fn new(channel: &'static TxChannelType) -> FakeTx {
        Self { channel }
    }
}

impl Tx for FakeTx {
    async fn write(&mut self, buffer: &[u8]) -> common::Result<()> {
        let s = core::str::from_utf8(buffer).map_err(|_| Error::StringEncodingError)?;
        let s = String::from_str(s).map_err(|_| Error::BufferTooSmallError)?;
        Ok(self.channel.send(s).await)
    }
}

static EXECUTOR: StaticCell<Executor> = StaticCell::new();

#[test]
fn uart_test() {
    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner| {
        spawner.must_spawn(main(spawner));
    });
}

#[embassy_executor::task]
async fn main(spawner: Spawner) {
    let rx = FakeRxWithIdle::new(vec![("ATI\r", "Fake modem\r\nOK")]);
    let tx = FakeTx::new(&TX_CHANNEL);
    let handler = |_: &str, _: &str| false;
    let mut at_uart = AtUart::new(rx, tx, handler, &spawner);

    let response = at_uart.call_at("I", Duration::from_millis(100)).await.unwrap();
    assert_eq!(
        response.lines(),
        &[
            FromModem::Line(String::from_str("Fake modem").unwrap()),
            FromModem::Ok
        ]
    );
    std::process::exit(0); // TODO: this is ugly, is there a better way?
}
