use chrono::{DateTime, FixedOffset};
use yaroc_common::at::response::CommandResponse;
use yaroc_common::bg77_hw::{Bg77, FakePin, ModemHw};
use yaroc_common::status::CellNetworkType;

use embassy_executor::{Executor, Spawner};
use embassy_sync::channel::Channel;
use heapless::Vec;
use static_cell::StaticCell;
use yaroc_common::at::uart::{FakeRxWithIdle, FakeTx, TxChannelType};
use yaroc_common::system_info::{BATTERY, SystemInfo, TEMPERATURE};

static EXECUTOR: StaticCell<Executor> = StaticCell::new();

#[test]
fn bg77_sysinfo_test() {
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
            ("AT+QLTS=2\r", "+QLTS: \"2024/12/24,10:48:23+04,0\"\r\nOK"),
            ("AT+QCSQ\r", "+QCSQ: \"NBIoT\",-107,-134,35,-20\r\nOK"),
            ("AT+QCFG=\"celevel\"\r", "+QCFG: \"celevel\",1\r\nOK"),
            ("AT+CEREG?\r", "+CEREG: 2,1,\"2008\",\"2B2078\",9\r\nOK"),
        ]),
        &TX_CHANNEL,
    );
    let tx = FakeTx::new(&TX_CHANNEL);

    let mut bg77 = Bg77::new(tx, rx, FakePin {}, Default::default());
    let handler = |_: &CommandResponse| false;
    bg77.spawn(handler, spawner);
    TEMPERATURE.sender().send(27.0);
    BATTERY.sender().send(yaroc_common::system_info::BatteryInfo {
        mv: 3967,
        percents: 76,
    });
    let mut send_punch = SystemInfo::default();

    let mch = send_punch.mini_call_home(&mut bg77).await.unwrap();
    let signal_info = mch.signal_info.unwrap();
    assert_eq!(signal_info.network_type, CellNetworkType::NbIotEcl1);
    assert_eq!(signal_info.rssi_dbm, -107);
    assert_eq!(signal_info.snr_cb, -130);
    assert_eq!(
        signal_info.cellid,
        Some(u32::from_str_radix("2B2078", 16).unwrap())
    );
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

    std::process::exit(0); // TODO: this is ugly, is there a better way?
}
