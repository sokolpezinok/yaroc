// Note: if the test is successful it ends with: "Error: CPU halted unexpectedly."
// This is caused by the final call to `asm::bkpt()`. A better solution is needed.
#![no_std]
#![no_main]

use chrono::{DateTime, FixedOffset};
use yaroc_nrf52840 as _;
use yaroc_nrf52840::bg77::{MqttConfig, BG77};
use yaroc_nrf52840::status::FakeTemp;

use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_sync::channel::Channel;
use heapless::Vec;
use yaroc_common::at::uart::{FakeRxWithIdle, FakeTx, TxChannelType};

#[embassy_executor::main]
async fn mini_call_home(spawner: Spawner) {
    static TX_CHANNEL: TxChannelType = Channel::new();
    let p = embassy_nrf::init(Default::default());
    let rx = FakeRxWithIdle::new(
        Vec::from_array([
            ("AT+QLTS=2\r", "+QLTS: \"2024/12/24,10:48:23+04,0\"\r\nOK"),
            ("AT+CBC\r", "+CBC: 0,76,3967\r\nOK"),
            ("AT+QCSQ\r", "+QCSQ: \"NBIoT\",-107,-134,35,-20\r\nOK"),
            ("AT+CEREG?\r", "+CEREG: 2,1,\"2008\",\"2B2078\",9\r\nOK"),
        ]),
        &TX_CHANNEL,
    );
    let tx = FakeTx::new(&TX_CHANNEL);
    let modem_pin = Output::new(p.P0_17, Level::Low, OutputDrive::Standard);

    let temp = FakeTemp { t: 27.0 };
    let mqtt_config = MqttConfig::default();
    let mut bg77 = BG77::new(rx, tx, modem_pin, temp, &spawner, mqtt_config);

    let mch = bg77.mini_call_home().await.unwrap();
    assert_eq!(mch.cellid, Some(u32::from_str_radix("2B2078", 16).unwrap()));
    assert_eq!(mch.rssi_dbm, Some(-107));
    assert_eq!(mch.snr_cb, Some(-130));
    assert_eq!(mch.batt_mv, Some(3967));
    assert_eq!(mch.batt_percents, Some(76));
    assert_eq!(mch.cpu_temperature, Some(27.0));
    assert_eq!(
        mch.timestamp,
        DateTime::<FixedOffset>::parse_from_str(
            "2024-12-24 10:48:23+01:00",
            "%Y-%m-%d %H:%M:%S%:z"
        )
        .unwrap()
    );

    defmt::info!("Test OK");
    cortex_m::asm::bkpt();
}
