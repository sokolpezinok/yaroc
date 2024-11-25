use chrono::prelude::*;

use crate::{
    error::Error,
    proto::{status, MiniCallHome as MiniCallHomeProto, Status, Timestamp},
};

pub fn parse_cclk(modem_clock: &str) -> Result<DateTime<FixedOffset>, Error> {
    let naive_date = NaiveDateTime::parse_from_str(&modem_clock[..17], "%y/%m/%d,%H:%M:%S")
        .map_err(|_| Error::ParseError)?;

    let offset = str::parse::<u8>(&modem_clock[18..]).map_err(|_| Error::ParseError)?;
    Ok(naive_date
        .and_local_timezone(FixedOffset::east_opt(i32::from(offset) * 900).unwrap())
        .unwrap()
        .fixed_offset())
}

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
    pub fn to_proto(&self, timestamp: Option<DateTime<FixedOffset>>) -> Status {
        Status {
            msg: Some(status::Msg::MiniCallHome(MiniCallHomeProto {
                freq: 32,
                millivolts: self.batt_mv.unwrap_or_default(),
                signal_dbm: self.rssi_dbm.unwrap_or_default() as i32,
                signal_snr: self.snr_db.unwrap_or_default() as i32,
                cellid: self.cellid.unwrap_or_default(),
                time: timestamp.map(to_timestamp),
                cpu_temperature: self.cpu_temperature.unwrap_or_default() as f32,
                ..Default::default()
            })),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod test {
    use chrono::{NaiveDate, NaiveTime};

    use super::parse_cclk;

    #[test]
    fn test_cclk() {
        let dt = parse_cclk("24/11/25,22:12:11+04").unwrap();
        let naive_dt = dt.naive_local();
        assert_eq!(
            naive_dt.date(),
            NaiveDate::from_ymd_opt(2024, 11, 25).unwrap()
        );
        assert_eq!(
            naive_dt.time(),
            NaiveTime::from_hms_opt(22, 12, 11).unwrap()
        );
        assert_eq!(dt.offset().local_minus_utc(), 3600);
    }
}
