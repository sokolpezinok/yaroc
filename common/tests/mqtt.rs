use embassy_executor::Spawner;
use embassy_futures::block_on;
use embassy_time::Duration;
use mockall::{Predicate, predicate::*};
use yaroc_common::at::response::{AtResponse, CommandResponse, FromModem};

use yaroc_common::at::uart::UrcHandlerType;
use yaroc_common::bg77::hw::{ACTIVATION_TIMEOUT, ModemHw};
use yaroc_common::bg77::mqtt::{MqttClient, MqttConfig};

// mockall::automock doesn't work next to `trait ModemHw` definition, so we use `mockall::mock!`
// instead.
mockall::mock! {
    pub ModemHw {}
    impl ModemHw for ModemHw {
        const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1);
        fn spawn(&mut self, spawner: Spawner, urc_handlers: &[UrcHandlerType]);
        async fn call_at(
            &mut self,
            cmd: &str,
            response_timeout: Option<Duration>,
        ) -> yaroc_common::Result<AtResponse>;
        async fn long_call_at(
            &mut self,
            cmd: &str,
            timeout: Duration,
        ) -> yaroc_common::Result<AtResponse>;
        async fn call(
            &mut self,
            msg: &[u8],
            command_prefix: &str,
            second_read_timeout: Option<Duration>,
        ) -> yaroc_common::Result<AtResponse>;
        async fn read(&mut self) -> yaroc_common::Result<AtResponse>;
        async fn turn_on(&mut self) -> yaroc_common::Result<()>;
    }
}

fn expect_at(
    mock: &mut MockModemHw,
    cmd_matcher: impl Predicate<str> + Send + 'static,
    timeout_matcher: impl Predicate<Option<Duration>> + Send + 'static,
    response: Option<&'static str>,
) {
    mock.expect_call_at()
        .with(cmd_matcher, timeout_matcher)
        .times(1)
        .returning(move |cmd, _| {
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

#[test]
fn test_mqtt_connect_ok() {
    let mut bg77 = MockModemHw::new();

    expect_at(&mut bg77, eq("+CGATT?"), eq(None), Some("+CGATT: 1"));
    expect_at(&mut bg77, eq("+QMTOPEN?"), eq(None), None);
    expect_at(
        &mut bg77,
        eq("+QMTCFG=\"timeout\",1,35,2,1"),
        eq(None),
        None,
    );
    expect_at(&mut bg77, eq("+QMTCFG=\"keepalive\",1,70"), eq(None), None);
    expect_at(
        &mut bg77,
        eq("+QMTOPEN=1,\"broker.emqx.io\",1883"),
        eq(Some(ACTIVATION_TIMEOUT)),
        Some("+QMTOPEN: 1,0"),
    );
    expect_at(&mut bg77, eq("+QMTCONN?"), eq(None), Some("+QMTCONN: 1,1"));
    expect_at(
        &mut bg77,
        str::starts_with("+QMTCONN=1,\"nrf52840-"),
        always(),
        Some("+QMTCONN: 1,0,0"),
    );

    let mut client = MqttClient::new(MqttConfig::default(), 1);
    assert!(block_on(client.mqtt_connect(&mut bg77)).is_ok());
}
