use core::str::FromStr;
use embassy_executor::{Executor, Spawner};
use embassy_sync::channel::Channel;
use embassy_time::Duration;
use heapless::{String, Vec};
use static_cell::StaticCell;
use yaroc_common::at::response::{CommandResponse, FromModem};
use yaroc_common::at::uart::{AtUartTrait, FakeRxWithIdle, MAIN_RX_CHANNEL, TxChannelType};
use yaroc_common::{at::uart::AtUart, error::Error};

static EXECUTOR: StaticCell<Executor> = StaticCell::new();
static URC_CHANNEL: Channel<yaroc_common::RawMutex, CommandResponse, 1> = Channel::new();

fn urc_handler(response: &CommandResponse) -> bool {
    if response.command() == "URC" {
        URC_CHANNEL.try_send(response.clone()).unwrap();
        true
    } else {
        false
    }
}

#[test]
fn uart_test() {
    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner| {
        spawner.must_spawn(main(spawner));
    });
}

#[embassy_executor::task]
async fn main(spawner: Spawner) {
    static TX_CHANNEL: TxChannelType = Channel::new();
    let rx = FakeRxWithIdle::new(
        Vec::from_array([
            ("ATI\r", "Fake modem\r\n+URC: 123\r\nOK"),
            ("AT+QMTOPEN=0,\"broker.com\",1883\r", "OK\r\n+QMTOPEN: 0,3"),
            ("AT+CBC\r", "ERROR"),
            ("AT+QCSQ\r", "Text"),
            ("AT+CEREG?\r", ""),
        ]),
        &TX_CHANNEL,
    );
    let mut at_uart = AtUart::new(&TX_CHANNEL, rx);
    at_uart.spawn_rx(&[urc_handler], spawner);

    let response = at_uart.call_at_timeout("I", Duration::from_millis(10), None).await.unwrap();
    assert_eq!(
        response.lines(),
        &[
            FromModem::Line(String::from_str("Fake modem").unwrap()),
            FromModem::Ok
        ]
    );

    let urc = URC_CHANNEL.try_receive().unwrap();
    assert_eq!(urc.command(), "URC");
    assert_eq!(urc.values(), ["123"]);

    let response = at_uart
        .call_at_timeout(
            "+QMTOPEN=0,\"broker.com\",1883",
            Duration::from_millis(10),
            Some(Duration::from_millis(10)),
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

    let error = at_uart.call_at_timeout("+CBC", Duration::from_millis(10), None).await.err();
    assert_eq!(error, Some(Error::AtErrorResponse));

    let error = at_uart.call_at_timeout("+QCSQ", Duration::from_millis(10), None).await.err();
    assert_eq!(error, Some(Error::ModemError));

    let error = at_uart.call_at_timeout("+CEREG?", Duration::from_millis(10), None).await.err();
    assert_eq!(error, Some(Error::TimeoutError));

    assert_eq!(MAIN_RX_CHANNEL.len(), 0);
    std::process::exit(0); // TODO: this is ugly, is there a better way?
}
