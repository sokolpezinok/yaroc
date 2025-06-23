use chrono::{prelude::*, Days};
use heapless::Vec;

use crate::{error::Error, proto::Punch};

pub const LEN: usize = 20;
pub type RawPunch = [u8; LEN];

#[derive(Debug, Clone, PartialEq)]
pub struct SiPunch {
    pub card: u32,
    pub code: u16,
    pub time: DateTime<FixedOffset>,
    pub mode: u8,
    pub raw: RawPunch,
}

const EARLY_SERIES_COMPLEMENT: u32 = 100_000 - (1 << 16);
const BILLION_BY_256: u32 = 1_000_000_000 / 256; // An integer

impl SiPunch {
    pub fn new(card: u32, code: u16, time: DateTime<FixedOffset>, mode: u8) -> Self {
        Self {
            card,
            code,
            time,
            mode,
            raw: Self::punch_to_bytes(card, code, time.naive_local(), mode),
        }
    }

    pub fn from_raw(bytes: RawPunch, today: NaiveDate, offset: &FixedOffset) -> Self {
        let data = &bytes[4..19];
        let code = u16::from_be_bytes([data[0] & 1, data[1]]);
        let mut card = u32::from_be_bytes(data[2..6].try_into().unwrap()) & 0xffffff;
        let series = card / (1 << 16);
        if series <= 4 {
            card += series * EARLY_SERIES_COMPLEMENT;
        }
        let data = &data[6..];
        let datetime = offset.from_local_datetime(&Self::bytes_to_datetime(data, today)).unwrap();

        Self {
            card,
            code,
            time: datetime,
            mode: data[4] & 0b1111,
            raw: bytes,
        }
    }

    pub fn to_proto(&self) -> Punch<'_> {
        Punch {
            raw: &self.raw,
            unknown_fields: femtopb::UnknownFields::empty(),
        }
    }

    pub fn punches_from_payload(
        payload: &[u8],
        today: NaiveDate,
        offset: &FixedOffset,
    ) -> Vec<Result<Self, Error>, 10> {
        payload
            .chunks(LEN)
            .map(|chunk| {
                let partial_payload: RawPunch = chunk.try_into().map_err(|_| {
                    //    format!("Wrong length of chunk={}", chunk.len()),
                    Error::BufferTooSmallError
                })?;
                Ok(Self::from_raw(partial_payload, today, offset))
            })
            .collect()
    }

    pub fn last_dow(dow: u32, today: NaiveDate) -> NaiveDate {
        assert!(dow <= 7);
        let days = (today.weekday().num_days_from_sunday() + 7 - dow) % 7;
        today - Days::new(u64::from(days))
    }

    fn bytes_to_datetime(data: &[u8], today: NaiveDate) -> NaiveDateTime {
        let dow = u32::from((data[0] & 0b1110) >> 1);
        let date = Self::last_dow(dow, today);

        let seconds: u32 = u32::from(data[0] & 1) * (12 * 60 * 60)
            + u32::from(u16::from_be_bytes(data[1..3].try_into().unwrap()));
        let nanos = u32::from(data[3]) * BILLION_BY_256;
        let time = NaiveTime::from_num_seconds_from_midnight_opt(seconds, nanos).unwrap();
        NaiveDateTime::new(date, time)
    }

    /// Reimplementation of Sportident checksum algorithm in Rust
    ///
    /// Note that they call it CRC but it is buggy. See the last test that leads to a checksum of 0 for
    /// a polynomial that's not divisible by 0x8005.
    fn sportident_checksum(message: &[u8]) -> u16 {
        let mut msg: Vec<u8, LEN> = Vec::from_slice(message).unwrap();
        msg.push(0).unwrap();
        if msg.len() % 2 == 1 {
            msg.push(0).unwrap();
        }

        let mut chksum = u16::from_be_bytes(msg[..2].try_into().unwrap());
        for i in (2..message.len()).step_by(2) {
            let mut val = u16::from_be_bytes(msg[i..i + 2].try_into().unwrap());
            for _ in 0..16 {
                if chksum & 0x08000 > 0 {
                    chksum <<= 1;
                    if val & 0x8000 > 0 {
                        chksum += 1;
                    }
                    chksum ^= 0x8005;
                } else {
                    chksum <<= 1;
                    if val & 0x8000 > 0 {
                        chksum += 1;
                    }
                }
                val <<= 1;
            }
        }
        chksum
    }

    /// Convert SportIdent card number to bytes.
    fn card_to_bytes(mut card: u32) -> [u8; 4] {
        let series = card / 100_000;
        if series <= 4 {
            card -= series * EARLY_SERIES_COMPLEMENT;
        }
        card.to_be_bytes()
    }

    /// Convert a timestamp to SportIdent 4-byte time representation.
    fn time_to_bytes(time: NaiveDateTime) -> [u8; 4] {
        let mut res = [0; 4];
        res[0] = u8::try_from(time.weekday().num_days_from_sunday()).unwrap() << 1;
        let secs = if time.hour() >= 12 {
            res[0] |= 1;
            const HALF_DAY_SECS: u32 = 12 * 60 * 60;
            time.num_seconds_from_midnight() - HALF_DAY_SECS
        } else {
            time.num_seconds_from_midnight()
        };

        let secs = u16::try_from(secs).unwrap().to_be_bytes();
        res[1..3].copy_from_slice(&secs);
        res[3] = u8::try_from(time.nanosecond() / BILLION_BY_256).unwrap();
        res
    }

    fn punch_to_bytes(card: u32, code: u16, time: NaiveDateTime, mode: u8) -> RawPunch {
        let mut res = [0; LEN];
        res[..4].copy_from_slice(&[0xff, 0x02, 0xd3, 0x0d]);
        res[4..6].copy_from_slice(&code.to_be_bytes());
        res[6..10].copy_from_slice(&Self::card_to_bytes(card));
        res[10..14].copy_from_slice(&Self::time_to_bytes(time));
        res[14] = mode;
        // res[15..17] is set to 0 out of 1, corresponding to the setting "send last punch"
        res[16] = 1;
        let chksum = Self::sportident_checksum(&res[2..17]).to_be_bytes();
        res[17..19].copy_from_slice(&chksum);
        res[19] = 0x03;
        res
    }
}

#[cfg(test)]
mod test_checksum {
    use super::SiPunch;

    #[test]
    fn test_checksum() {
        let s = b"\xd3\r\x00\x02\x00\x1f\xb5\xf3\x18\x99As\x00\x07\x08";
        assert_eq!(SiPunch::sportident_checksum(s), 0x8f98);

        let s = b"\xd3\r\x00\x02\x00\x1f\xb5\xf3\x18\x9b\x98\x1e\x00\x070";
        assert_eq!(SiPunch::sportident_checksum(s), 0x4428);

        let s = b"\x01\x80\x05";
        assert_eq!(SiPunch::sportident_checksum(s), 0);
    }
}

#[cfg(test)]
mod test_punch {
    use chrono::prelude::*;

    use crate::{error::Error, punch::SiPunch};

    #[test]
    fn test_card_series() {
        let bytes = SiPunch::card_to_bytes(65535);
        assert_eq!(bytes, [0, 0x00, 0xff, 0xff]);

        let bytes = SiPunch::card_to_bytes(416534);
        assert_eq!(bytes, [0, 0x04, 0x40, 0x96]);

        let bytes = SiPunch::card_to_bytes(81110151);
        assert_eq!(bytes, [4, 0xd5, 0xa4, 0x87]);
    }

    #[test]
    fn test_time_to_bytes() {
        let time = NaiveDateTime::parse_from_str("2023-11-23 10:00:03.793", "%Y-%m-%d %H:%M:%S%.f")
            .unwrap();
        let bytes = SiPunch::time_to_bytes(time);
        assert_eq!(bytes, [0x8, 0x8c, 0xa3, 0xcb]);

        let time = NaiveDateTime::parse_from_str("2023-11-23 10:00:03.999", "%Y-%m-%d %H:%M:%S%.f")
            .unwrap();
        let bytes = SiPunch::time_to_bytes(time);
        assert_eq!(bytes, [0x8, 0x8c, 0xa3, 0xff]);

        let time =
            NaiveDateTime::parse_from_str("2023-11-23 10:00:03.0", "%Y-%m-%d %H:%M:%S%.f").unwrap();
        let bytes = SiPunch::time_to_bytes(time);
        assert_eq!(bytes, [0x8, 0x8c, 0xa3, 0x00]);
    }

    #[test]
    fn test_punch() {
        let time = DateTime::parse_from_rfc3339("2023-11-23T10:00:03.793+01:00").unwrap();
        let punch = SiPunch::new(1715004, 47, time, 2).raw;
        assert_eq!(
            &punch,
            b"\xff\x02\xd3\x0d\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\x50\xe3\x03"
        );
    }

    #[test]
    fn test_punches_from_payload() {
        let time = DateTime::parse_from_rfc3339("2023-11-23T10:00:03.792968750+01:00").unwrap();

        let punch = SiPunch::new(1715004, 47, time, 2);
        let payload =
            b"\xff\x02\xd3\x0d\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\x50\xe3\x03\xff\x02";

        let punches = SiPunch::punches_from_payload(payload, time.date_naive(), time.offset());
        assert_eq!(punches.len(), 2);
        assert_eq!(*punches[0].as_ref().unwrap(), punch);
        assert_eq!(
            *punches[1].as_ref().unwrap_err(),
            Error::BufferTooSmallError
        );
    }
}
