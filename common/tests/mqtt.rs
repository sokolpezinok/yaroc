use embassy_executor::Spawner;
use embassy_futures::block_on;
use embassy_time::Duration;
use mockall::{Predicate, predicate::*};

use yaroc_common::Result;
use yaroc_common::at::response::{AT_LINES, AtResponse, CommandResponse, FromModem};
use yaroc_common::at::uart::{AtUartTrait, UrcHandlerType};
use yaroc_common::bg77::hw::ModemHw;
use yaroc_common::bg77::modem_manager::ACTIVATION_TIMEOUT;
use yaroc_common::bg77::mqtt::{MqttClient, MqttConfig, MqttQos};

// mockall::automock doesn't work next to `trait ModemHw` definition, so we use `mockall::mock!`
// instead.
mockall::mock! {
    pub AtUart {}
    impl AtUartTrait for AtUart {
        fn spawn_rx(&mut self, urc_handlers: &[UrcHandlerType], spawner: Spawner);
        async fn call_at_timeout(
            &mut self,
            command: &str,
            call_timeout: Duration,
            response_timeout: Option<Duration>,
        ) -> Result<AtResponse>;
        async fn call_second_read(
            &mut self,
            msg: &[u8],
            command_prefix: &str,
            second_read: bool,
            timeout: Duration,
        ) -> Result<AtResponse>;
        async fn read(
            &self,
            timeout: Duration,
        ) -> Result<heapless::Vec<FromModem, AT_LINES>>;
    }
}

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1);

fn expect_call_at(
    mock: &mut MockAtUart,
    cmd_matcher: impl Predicate<str> + Send + 'static,
    response_timeout_matcher: impl Predicate<Option<Duration>> + Send + 'static,
    response: Option<&'static str>,
) {
    mock.expect_call_at_timeout()
        .with(cmd_matcher, eq(DEFAULT_TIMEOUT), response_timeout_matcher)
        .times(1)
        .returning(move |cmd, _, _| {
            let mut resps = heapless::Vec::new();
            if let Some(r) = response {
                resps
                    .push(FromModem::CommandResponse(CommandResponse::new(r).unwrap()))
                    .unwrap();
            }
            resps.push(FromModem::Ok).unwrap();
            Ok(AtResponse::new(resps, cmd))
        });
}

impl ModemHw for MockAtUart {
    const DEFAULT_TIMEOUT: Duration = DEFAULT_TIMEOUT;
}

#[test]
fn test_mqtt_connect_ok() {
    let mut bg77 = MockAtUart::new();

    expect_call_at(&mut bg77, eq("+CGATT?"), eq(None), Some("+CGATT: 1"));
    expect_call_at(&mut bg77, eq("+QMTOPEN?"), eq(None), None);
    expect_call_at(
        &mut bg77,
        eq("+QMTCFG=\"timeout\",1,35,2,1"),
        eq(None),
        None,
    );
    expect_call_at(&mut bg77, eq("+QMTCFG=\"keepalive\",1,70"), eq(None), None);
    expect_call_at(
        &mut bg77,
        eq("+QMTOPEN=1,\"broker.emqx.io\",1883"),
        eq(Some(ACTIVATION_TIMEOUT)),
        Some("+QMTOPEN: 1,0"),
    );
    expect_call_at(&mut bg77, eq("+QMTCONN?"), eq(None), Some("+QMTCONN: 1,1"));
    expect_call_at(
        &mut bg77,
        str::starts_with("+QMTCONN=1,\"nrf52840-"),
        always(),
        Some("+QMTCONN: 1,0,0"),
    );

    let mut client = MqttClient::new(MqttConfig::default(), 1);
    assert!(block_on(client.connect(&mut bg77)).is_ok());
}

#[test]
fn test_mqtt_send_short_message_ok() {
    let mut bg77 = MockAtUart::new();
    let topic = "topic";
    let message = b"hello";

    expect_call_at(
        &mut bg77,
        // Note: The `5` here is the message length.
        eq("+QMTPUB=1,0,0,0,\"yar/deadbeef/topic\",5"),
        eq(None),
        None,
    );

    // Expect the payload to be sent via call_second_read
    bg77.expect_call_second_read()
        .with(
            eq(message.as_slice()),
            eq("+QMTPUB"),
            eq(true),
            eq(Duration::from_secs(5)),
        )
        .times(1)
        .returning(|_, _, _, _| {
            let resps = [FromModem::CommandResponse(
                CommandResponse::new("+QMTPUB: 0,0,0").unwrap(),
            )]
            .into();
            Ok(AtResponse::new(resps, "+QMTPUB"))
        });

    let mut client = MqttClient::new(MqttConfig::default(), 1);
    assert!(block_on(client.send_message(&mut bg77, topic, message, MqttQos::Q0, 0)).is_ok());
}
