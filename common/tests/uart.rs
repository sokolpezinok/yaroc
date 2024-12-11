use embassy_executor::{Executor, Spawner};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::Duration;
use heapless::String;
use static_cell::StaticCell;
use std::str::FromStr;
use yaroc_common::at::response::{CommandResponse, FromModem, AT_COMMAND_SIZE};
use yaroc_common::at::uart::{AtRxBroker, RxWithIdle, Tx, MAIN_RX_CHANNEL};
use yaroc_common::{at::uart::AtUart, error::Error};

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
async fn reader(rx: FakeRxWithIdle, urc_classifier: fn(&CommandResponse) -> bool) {
    AtRxBroker::broker_loop(rx, urc_classifier, &MAIN_RX_CHANNEL).await;
}

impl RxWithIdle for FakeRxWithIdle {
    fn spawn(self, spawner: &Spawner, urc_classifier: fn(&CommandResponse) -> bool) {
        spawner.must_spawn(reader(self, urc_classifier))
    }

    async fn read_until_idle(&mut self, buf: &mut [u8]) -> yaroc_common::Result<usize> {
        let recv_command = TX_CHANNEL.receive().await;
        if let Some((command, response)) = self.responses.first() {
            assert_eq!(command, &recv_command);
            let bytes = response.as_bytes();
            buf[..bytes.len()].clone_from_slice(bytes);
            self.responses.remove(0);
            Ok(bytes.len())
        } else {
            Err(yaroc_common::error::Error::TimeoutError)
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
    async fn write(&mut self, buffer: &[u8]) -> yaroc_common::Result<()> {
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
    let rx = FakeRxWithIdle::new(vec![
        ("ATI\r", "Fake modem\r\nOK"),
        ("AT+QMTOPEN=0,\"broker.com\",1883\r", "OK\r\n+QMTOPEN: 0,3"),
        ("AT+CBC\r", "ERROR"),
        ("AT+QCSQ\r", "Text"),
    ]);
    let tx = FakeTx::new(&TX_CHANNEL);
    let classifier = |_: &CommandResponse| false;
    let mut at_uart = AtUart::new(rx, tx, classifier, &spawner);

    let response = at_uart.call_at("I", Duration::from_millis(10)).await.unwrap();
    assert_eq!(
        response.lines(),
        &[
            FromModem::Line(String::from_str("Fake modem").unwrap()),
            FromModem::Ok
        ]
    );

    let response = at_uart
        .call_at_with_response(
            "+QMTOPEN=0,\"broker.com\",1883",
            Duration::from_millis(10),
            Duration::from_millis(10),
        )
        .await
        .unwrap();
    assert_eq!(
        response.lines(),
        &[
            FromModem::Ok,
            FromModem::CommandResponse(CommandResponse::new("+QMTOPEN: 0,3").unwrap()),
            FromModem::Eof,
        ]
    );

    let error = at_uart.call_at("+CBC", Duration::from_millis(10)).await.err();
    assert_eq!(error, Some(Error::AtErrorResponse));

    let error = at_uart.call_at("+QCSQ", Duration::from_millis(10)).await.err();
    assert_eq!(error, Some(Error::ModemError));

    assert_eq!(MAIN_RX_CHANNEL.len(), 0);
    std::process::exit(0); // TODO: this is ugly, is there a better way?
}
