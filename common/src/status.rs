use chrono::prelude::*;
use femtopb::UnknownFields;

use crate::proto::{status, MiniCallHome, Status, Timestamp};

fn to_timestamp<'a>(time: DateTime<FixedOffset>) -> Timestamp<'a> {
    Timestamp {
        millis_epoch: time.timestamp_millis() as u64,
        unknown_fields: UnknownFields::empty(),
    }
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct SignalInfo {
    pub rssi_dbm: Option<i8>,
    pub snr_db: Option<f32>,
    pub cellid: Option<u32>,
}

impl SignalInfo {
    pub fn to_proto(&self, timestamp: Option<NaiveDateTime>) -> Status {
        Status {
            msg: Some(status::Msg::MiniCallHome(MiniCallHome {
                freq: 32,
                volts: 3.9,
                signal_dbm: self.rssi_dbm.unwrap_or_default() as i32,
                signal_snr: self.snr_db.unwrap_or_default() as i32,
                cellid: self.cellid.unwrap_or_default(),
                time: timestamp.map(|t| to_timestamp(t.and_utc().fixed_offset())), // TODO
                ..Default::default()
            })),
            unknown_fields: UnknownFields::empty(),
        }
    }
}
