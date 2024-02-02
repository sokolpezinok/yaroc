use std::fmt;

use crate::logs::HostInfo;
use crate::protobufs::Punch;
use chrono::{prelude::*, Days, Duration};
use meshtastic::protobufs::mesh_packet::PayloadVariant;
use meshtastic::protobufs::PortNum;
use meshtastic::protobufs::{Data, ServiceEnvelope};
use meshtastic::Message as MeshtasticMessage;
use pyo3::prelude::*;

#[derive(Debug)]
#[pyclass]
pub struct SiPunch {
    #[pyo3(get)]
    card: u32,
    #[pyo3(get)]
    pub code: u16,
    #[pyo3(get)]
    pub time: DateTime<FixedOffset>,
    #[pyo3(get)]
    mode: u8,
    #[pyo3(get)]
    pub host_info: HostInfo,
    latency: Duration,
    #[pyo3(get)]
    raw: [u8; 20],
}

const EARLY_SERIES_COMPLEMENT: u32 = 100_000 - (1 << 16);
const BILLION_BY_256: u32 = 1_000_000_000 / 256; // An integer

#[pymethods]
impl SiPunch {
    #[staticmethod]
    pub fn new(
        card: u32,
        code: u16,
        time: DateTime<FixedOffset>,
        mode: u8,
        host_info: HostInfo,
    ) -> Self {
        Self {
            card,
            code,
            time,
            latency: Local::now().fixed_offset() - time,
            mode,
            host_info,
            raw: Self::punch_to_bytes(code, time, card, mode),
        }
    }

    #[staticmethod]
    pub fn from_raw(payload: [u8; 20], host_info: HostInfo) -> Self {
        let data = &payload[4..19];
        let code = u16::from_be_bytes([data[0] & 1, data[1]]);
        let mut card = u32::from_be_bytes(data[2..6].try_into().unwrap()) & 0xffffff;
        let series = card / (1 << 16);
        if series >= 1 && series <= 4 {
            card += series * EARLY_SERIES_COMPLEMENT;
        }
        let data = &data[6..];
        let datetime = Self::bytes_to_datetime(data);

        Self {
            card,
            code,
            time: datetime,
            latency: Local::now().fixed_offset() - datetime,
            mode: data[4] & 0b1111,
            host_info: host_info.clone(),
            raw: payload,
        }
    }

    pub fn __repr__(&self) -> String {
        format!("{}", self)
    }
}

impl SiPunch {
    pub fn from_msh_serial(payload: &[u8]) -> Result<Self, std::io::Error> {
        let service_envelope = ServiceEnvelope::decode(payload)?;
        let packet = service_envelope.packet.expect("Packet missing");
        let mac_addr = format!("{:8x}", packet.from);
        const SERIAL_APP: i32 = PortNum::SerialApp as i32;
        match packet.payload_variant {
            Some(PayloadVariant::Decoded(Data {
                portnum: SERIAL_APP,
                payload,
                ..
            })) => Ok(Self::from_raw(
                payload.try_into().unwrap(),
                HostInfo {
                    name: String::new(),
                    mac_address: mac_addr.clone(),
                },
            )),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Encrypted message or wrong portnum",
            )),
        }
    }

    pub fn from_proto(punch: Punch, host_info: &HostInfo) -> Result<Self, Vec<u8>> {
        Ok(Self::from_raw(punch.raw.try_into()?, host_info.clone()))
    }

    fn bytes_to_datetime(data: &[u8]) -> DateTime<FixedOffset> {
        let dow = u32::from((data[0] & 0b1110) >> 1);
        let today = Local::now().date_naive();
        let days = (today.weekday().num_days_from_sunday() + 7 - dow) % 7;
        let date = today.checked_sub_days(Days::new(u64::from(days))).unwrap();

        let seconds: u32 = u32::from(data[0] & 1) * (12 * 60 * 60)
            + u32::from(u16::from_be_bytes(data[1..3].try_into().unwrap()));
        let nanos = u32::from(data[3]) * BILLION_BY_256;
        let time = NaiveTime::from_num_seconds_from_midnight_opt(seconds, nanos).unwrap();
        NaiveDateTime::new(date, time)
            .and_local_timezone(Local)
            .unwrap()
            .fixed_offset()
    }

    /// Reimplementation of Sportident checksum algorithm in Rust
    ///
    /// Note that they call it CRC but it is buggy. See the last test that leads to a checksum of 0 for
    /// a polynomial that's not divisible by 0x8005.
    fn sportident_checksum(message: &[u8]) -> u16 {
        let to_add = 2 - message.len() % 2;
        let suffix = vec![b'\x00'; to_add];
        let mut msg = Vec::from(message);
        msg.extend(suffix);

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

    fn card_to_bytes(mut card: u32) -> [u8; 4] {
        let series = card / 100_000;
        if series >= 1 && series <= 4 {
            card -= series * EARLY_SERIES_COMPLEMENT;
        }
        card.to_be_bytes()
    }

    fn time_to_bytes(time: DateTime<FixedOffset>) -> [u8; 4] {
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

    pub fn punch_to_bytes(code: u16, time: DateTime<FixedOffset>, card: u32, mode: u8) -> [u8; 20] {
        let mut res = [0; 20];
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

impl fmt::Display for SiPunch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} punched {} ",
            self.host_info.name, self.card, self.code
        )?;
        write!(f, "at {}", self.time.format("%H:%M:%S.%3f"))?;
        let millis = self.latency.num_milliseconds() as f64 / 1000.0;
        write!(f, ", latency {:4.2}s", millis)
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

    use crate::{logs::HostInfo, punch::SiPunch};

    #[test]
    fn test_card_series() {
        let bytes = SiPunch::card_to_bytes(416534);
        assert_eq!(bytes, [0, 0x04, 0x40, 0x96]);

        let bytes = SiPunch::card_to_bytes(81110151);
        assert_eq!(bytes, [4, 0xd5, 0xa4, 0x87]);
    }

    #[test]
    fn test_time_to_bytes() {
        let time = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2023, 11, 23).unwrap(),
            NaiveTime::from_hms_milli_opt(10, 0, 3, 793).unwrap(),
        )
        .and_local_timezone(Local)
        .unwrap()
        .fixed_offset();
        let bytes = SiPunch::time_to_bytes(time);
        assert_eq!(bytes, [0x8, 0x8c, 0xa3, 0xcb]);

        let time = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2023, 11, 23).unwrap(),
            NaiveTime::from_hms_milli_opt(10, 0, 3, 999).unwrap(),
        )
        .and_local_timezone(Local)
        .unwrap()
        .fixed_offset();
        let bytes = SiPunch::time_to_bytes(time);
        assert_eq!(bytes, [0x8, 0x8c, 0xa3, 0xff]);
    }

    #[test]
    fn test_punch() {
        let tz = FixedOffset::east_opt(7200).unwrap();
        let time = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2023, 11, 23).unwrap(),
            NaiveTime::from_hms_milli_opt(10, 0, 3, 793).unwrap(),
        )
        .and_local_timezone(tz)
        .unwrap();
        let punch = SiPunch::punch_to_bytes(47, time, 1715004, 2);
        assert_eq!(
            &punch,
            b"\xff\x02\xd3\x0d\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\x50\xe3\x03"
        );
    }

    #[test]
    fn test_display() {
        let tz = FixedOffset::east_opt(7200).unwrap();
        let time = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2023, 11, 23).unwrap(),
            NaiveTime::from_hms_milli_opt(10, 0, 3, 793).unwrap(),
        )
        .and_local_timezone(tz)
        .unwrap();
        let host_info = HostInfo {
            name: "ROC1".to_owned(),
            mac_address: "abcdef123456".to_owned(),
        };
        let punch = SiPunch::new(46283, 47, time, 1, host_info);

        assert_eq!(format!("{punch}"), "ROC1 46283 punched 47 at 10:00:03.793");
    }
}
