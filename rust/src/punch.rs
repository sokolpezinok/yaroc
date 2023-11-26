/// Reimplementation of Sportident checksum algorithm in Rust
///
/// Note that they call it CRC but it is buggy. See the last test that leads to a checksum of 0 for
/// a polynomial that's not divisible by 0x8005.
pub fn sportident_checksum(message: &[u8]) -> u16 {
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
use chrono::prelude::*;

fn time_to_bytes(time: DateTime<Utc>) -> [u8; 4] {
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
    res[1] = secs[0];
    res[2] = secs[1];
    const MILLION_BY_256: u32 = 1_000_000_000 / 256; // An integer
    res[3] = u8::try_from(time.nanosecond() / MILLION_BY_256).unwrap();
    res
}

#[cfg(test)]
mod test_checksum {
    use super::sportident_checksum;

    #[test]
    fn test_checksum() {
        let s = b"\xd3\r\x00\x02\x00\x1f\xb5\xf3\x18\x99As\x00\x07\x08";
        assert_eq!(sportident_checksum(s), 0x8f98);

        let s = b"\xd3\r\x00\x02\x00\x1f\xb5\xf3\x18\x9b\x98\x1e\x00\x070";
        assert_eq!(sportident_checksum(s), 0x4428);

        let s = b"\x01\x80\x05";
        assert_eq!(sportident_checksum(s), 0);
    }
}

mod test_time {
    use std::time::Duration;

    use chrono::prelude::*;

    use super::time_to_bytes;

    #[test]
    fn test_time_to_bytes() {
        let date: DateTime<Utc> =
            Utc.with_ymd_and_hms(2023, 11, 23, 10, 0, 3).unwrap() + Duration::from_micros(792969);
        let bytes = time_to_bytes(date);
        assert_eq!(bytes, [0x8, 0x8c, 0xa3, 0xcb]);
    }
}
