use chrono::prelude::*;

use crate::proto::{status, MiniCallHome as MiniCallHomeProto, Status, Timestamp};

fn to_timestamp<'a>(time: DateTime<FixedOffset>) -> Timestamp<'a> {
    Timestamp {
        millis_epoch: time.timestamp_millis() as u64,
        ..Default::default()
    }
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct MiniCallHome {
    pub rssi_dbm: Option<i8>,
    pub snr_db: Option<f32>,
    pub cellid: Option<u32>,
    pub batt_mv: Option<u32>,
    pub batt_percents: Option<u8>,
    pub cpu_temperature: Option<i8>,
}

impl MiniCallHome {
    pub fn to_proto(&self, timestamp: Option<NaiveDateTime>) -> Status {
        Status {
            msg: Some(status::Msg::MiniCallHome(MiniCallHomeProto {
                freq: 32,
                millivolts: self.batt_mv.unwrap_or_default(),
                signal_dbm: self.rssi_dbm.unwrap_or_default() as i32,
                signal_snr: self.snr_db.unwrap_or_default() as i32,
                cellid: self.cellid.unwrap_or_default(),
                time: timestamp.map(|t| to_timestamp(t.and_utc().fixed_offset())), // TODO
                cpu_temperature: self.cpu_temperature.unwrap_or_default() as f32,
                ..Default::default()
            })),
            ..Default::default()
        }
    }
}
