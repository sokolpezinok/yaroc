use chrono::prelude::*;

use crate::{
    error::Error,
    proto::{status, MiniCallHome as MiniCallHomeProto, Status, Timestamp},
};

/// Parses the output of AT+QLTS=2 command into a date+time.
pub fn parse_qlts(modem_clock: &str) -> Result<DateTime<FixedOffset>, Error> {
    let naive_date = NaiveDateTime::parse_from_str(&modem_clock[..19], "%Y/%m/%d,%H:%M:%S")
        .map_err(|_| Error::ParseError)?;

    let offset = str::parse::<u8>(&modem_clock[20..22]).map_err(|_| Error::ParseError)?;
    Ok(naive_date
        .and_local_timezone(FixedOffset::east_opt(i32::from(offset) * 900).unwrap())
        .unwrap()
        .fixed_offset())
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Default)]
pub struct MiniCallHome {
    pub rssi_dbm: Option<i8>,
    pub snr_db: Option<f32>,
    pub cellid: Option<u32>,
    pub batt_mv: Option<u16>,
    pub batt_percents: Option<u8>,
    pub cpu_temperature: Option<f32>,
}

impl MiniCallHome {
    pub fn set_signal_info(&mut self, snr_mult: u8, mut rssi_dbm: i8, rsrp_dbm: i8, rsrq_dbm: i8) {
        let snr_db = f32::from(snr_mult) / 5. - 20.;
        self.snr_db = Some(snr_db);
        if rssi_dbm == 0 {
            rssi_dbm = rsrp_dbm - rsrq_dbm;
        }
        self.rssi_dbm = Some(rssi_dbm);
    }

    pub fn set_battery_info(&mut self, battery_mv: u16, battery_percents: u8) {
        self.batt_mv = Some(battery_mv);
        self.batt_percents = Some(battery_percents);
    }

    pub fn set_cpu_temperature(mut self, cpu_temperature: f32) -> Self {
        self.cpu_temperature = Some(cpu_temperature);
        self
    }

    pub fn set_cellid(&mut self, cellid: u32) {
        self.cellid = Some(cellid);
    }

    pub fn to_proto(&self, timestamp: Option<DateTime<FixedOffset>>) -> Status {
        Status {
            msg: Some(status::Msg::MiniCallHome(MiniCallHomeProto {
                freq: 32,
                millivolts: self.batt_mv.unwrap_or_default() as u32,
                signal_dbm: self.rssi_dbm.unwrap_or_default() as i32,
                signal_snr: self.snr_db.unwrap_or_default() as i32,
                cellid: self.cellid.unwrap_or_default(),
                time: timestamp.map(|t| Timestamp {
                    millis_epoch: t.timestamp_millis() as u64,
                    ..Default::default()
                }),
                cpu_temperature: self.cpu_temperature.unwrap_or_default(),
                ..Default::default()
            })),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod test {
    use chrono::{NaiveDate, NaiveTime};

    use super::parse_qlts;

    #[test]
    fn test_cclk() {
        let dt = parse_qlts("2024/11/25,22:12:11+04extra").unwrap();
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
