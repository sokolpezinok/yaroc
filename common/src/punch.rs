use chrono::{Days, prelude::*};
use heapless::Vec;

use crate::{error::Error, proto::Punch};

/// The length of a raw punch record in bytes.
pub const LEN: usize = 20;
/// A raw punch record as received from the SportIdent station.
pub type RawPunch = [u8; LEN];

/// A SportIdent punch, representing a single timestamped record from a control station.
///
/// This struct holds the decoded information from a raw SportIdent punch,
/// including the card number, control code, and time. It also keeps the original
/// raw data.
///
/// # Example
///
/// ```
/// use chrono::{NaiveDate, FixedOffset};
/// use yaroc_common::punch::SiPunch;
///
/// let raw_data = &[
///     0x03, 0xff, 0x02, 0xd3, 0x0d, 0x00, 0x2f, 0x00, 0x1a, 0x2b, 0x3c, 0x08, 0x8c, 0xa3, 0xcb,
///     0x02, 0x00, 0x01, 0x50, 0xe3, 0x03, 0xff, 0x02,
/// ];
/// let today = NaiveDate::from_ymd_opt(2023, 11, 23).unwrap();
/// let tz = FixedOffset::east_opt(3600).unwrap();
///
/// if let Some((raw_punch, rest)) = SiPunch::find_punch_data(raw_data) {
///     let punch = SiPunch::from_raw(raw_punch, today, &tz);
///     assert_eq!(punch.card, 1715004);
///     assert_eq!(punch.code, 47);
///     // Continue parsing `rest`
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct SiPunch {
    /// The card number of the SportIdent card that made the punch.
    pub card: u32,
    /// The control code of the station where the punch was made.
    pub code: u16,
    /// The timestamp of the punch, with a fixed timezone offset.
    pub time: DateTime<FixedOffset>,
    /// The punch mode, indicating the type of station (e.g., start, finish, control).
    pub mode: u8,
    /// The original 20-byte raw data from which this punch was parsed.
    pub raw: RawPunch,
}

const EARLY_SERIES_COMPLEMENT: u32 = 100_000 - (1 << 16);
/// Precision of SportIdent timing is 1/256 of a second.
const BILLION_BY_256: u32 = 1_000_000_000 / 256; // An integer
const HALF_DAY_SECS: u32 = 12 * 60 * 60;
const HEADER: [u8; 4] = [0xff, 0x02, 0xd3, 0x0d];
const FOOTER: u8 = 0x03;

impl SiPunch {
    /// Creates a new `SiPunch` from its components and serializes it into the raw byte format.
    ///
    /// # Arguments
    ///
    /// * `card` - The card number.
    /// * `code` - The control code.
    /// * `time` - The timestamp of the punch.
    /// * `mode` - The punch mode.
    ///
    /// # Returns
    ///
    /// A new `SiPunch` instance.
    pub fn new(card: u32, code: u16, time: DateTime<FixedOffset>, mode: u8) -> Self {
        Self {
            card,
            code,
            time,
            mode,
            raw: Self::punch_to_bytes(card, code, time.naive_local(), mode),
        }
    }

    /// Creates a new `SiPunch` by parsing a raw 20-byte punch record.
    ///
    /// # Arguments
    ///
    /// * `bytes` - The raw 20-byte punch data.
    /// * `today` - The current date, used to resolve the day of the week from the punch data.
    /// * `offset` - The timezone offset to apply to the punch time.
    ///
    /// # Returns
    ///
    /// A new `SiPunch` instance.
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

    /// Converts this `SiPunch` to its protobuf representation.
    pub fn to_proto(&self) -> Punch<'_> {
        Punch {
            raw: &self.raw,
            unknown_fields: femtopb::UnknownFields::empty(),
        }
    }

    /// Parses a byte slice containing one or more punch records.
    ///
    /// This function searches for punch data in the payload, decodes it, and returns a vector
    /// of `SiPunch` instances. It can handle cases where the payload contains partial or
    /// multiple punch records.
    ///
    /// # Arguments
    ///
    /// * `payload` - The byte slice to parse.
    /// * `today` - The current date, used for timestamp decoding.
    /// * `offset` - The timezone offset to apply.
    ///
    /// # Returns
    ///
    /// A `Vec` containing `Result<SiPunch, Error>`. `Ok` contains a successfully parsed punch,
    /// while `Err` indicates a parsing failure.
    pub fn punches_from_payload<const N: usize>(
        payload: &[u8],
        today: NaiveDate,
        offset: &FixedOffset,
    ) -> Vec<Result<Self, Error>, N> {
        match Self::find_punch_data(payload) {
            None => {
                let mut res = Vec::new();
                res.push(Err(Error::ValueError)).unwrap();
                res
            }
            Some((punch, rest)) => {
                let mut res = Vec::new();
                res.push(Ok(Self::from_raw(punch, today, offset))).unwrap();

                res.extend(rest.chunks(LEN).map(|chunk| {
                    let partial_payload: RawPunch =
                        chunk.try_into().map_err(|_| Error::BufferTooSmallError)?;
                    Ok(Self::from_raw(partial_payload, today, offset))
                }));
                res
            }
        }
    }

    /// Calculates the date of the most recent given day of the week.
    ///
    /// SportIdent punches encode the day of the week, not the full date. This function
    /// determines the full date by finding the most recent occurrence of that day of the week
    /// relative to `today`.
    ///
    /// # Arguments
    ///
    /// * `dow` - The day of the week (0=Sunday, 1=Monday, ..., 6=Saturday).
    /// * `today` - The current date.
    ///
    /// # Returns
    ///
    /// The `NaiveDate` of the last occurrence of the given day of the week.
    pub fn last_dow(dow: u8, today: NaiveDate) -> NaiveDate {
        assert!(dow <= 7);
        let days = (today.weekday().num_days_from_sunday() + 7 - u32::from(dow)) % 7;
        today - Days::new(u64::from(days))
    }

    /// Converts a 4-byte SportIdent time representation into a `NaiveDateTime`.
    ///
    /// The SportIdent time format consists of a day of the week, a 12-hour AM/PM indicator,
    /// seconds within that 12-hour period, and fractional seconds.
    ///
    /// # Arguments
    ///
    /// * `data` - A 4-byte slice containing the time data.
    /// * `today` - The current date, used to resolve the full date.
    ///
    /// # Returns
    ///
    /// The corresponding `NaiveDateTime`.
    fn bytes_to_datetime(data: &[u8], today: NaiveDate) -> NaiveDateTime {
        let dow = (data[0] & 0b1110) >> 1;
        let date = Self::last_dow(dow, today);

        // Lowest byte of data is a switch for AM/PM. data[1] and data[2] encode seconds within
        // each 12-hour period (AM or PM).
        let seconds: u32 = u32::from(data[0] & 1) * HALF_DAY_SECS
            + u32::from(u16::from_be_bytes([data[1], data[2]]));
        let nanos = u32::from(data[3]) * BILLION_BY_256;
        let time = NaiveTime::from_num_seconds_from_midnight_opt(seconds, nanos).unwrap();
        NaiveDateTime::new(date, time)
    }

    /// Implements the SportIdent checksum algorithm.
    ///
    /// Note: SportIdent calls this a CRC, but it has some unusual properties.
    /// The implementation is based on reverse-engineering the algorithm.
    fn sportident_checksum(message: &[u8]) -> u16 {
        let mut msg: Vec<u8, LEN> = Vec::from_slice(message).unwrap();
        msg.push(0).unwrap();
        if msg.len() % 2 == 1 {
            msg.push(0).unwrap();
        }

        let mut chksum = u16::from_be_bytes([msg[0], msg[1]]);
        for i in (2..message.len()).step_by(2) {
            let mut val = u16::from_be_bytes([msg[i], msg[i + 1]]);
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

    /// Converts a SportIdent card number to its 4-byte representation.
    ///
    /// This handles the encoding scheme for early card series (1-4) which have a
    /// different mapping.
    fn card_to_bytes(mut card: u32) -> [u8; 4] {
        let series = card / 100_000;
        if series <= 4 {
            card -= series * EARLY_SERIES_COMPLEMENT;
        }
        card.to_be_bytes()
    }

    /// Converts a `NaiveDateTime` to the 4-byte SportIdent time representation.
    fn time_to_bytes(time: NaiveDateTime) -> [u8; 4] {
        let mut res = [0; 4];
        res[0] = u8::try_from(time.weekday().num_days_from_sunday()).unwrap() << 1;
        let secs = if time.hour() >= 12 {
            res[0] |= 1;
            time.num_seconds_from_midnight() - HALF_DAY_SECS
        } else {
            time.num_seconds_from_midnight()
        };

        let secs = u16::try_from(secs).unwrap().to_be_bytes();
        res[1..3].copy_from_slice(&secs);
        res[3] = u8::try_from(time.nanosecond() / BILLION_BY_256).unwrap();
        res
    }

    /// Serializes a punch's components into a raw 20-byte SportIdent record.
    fn punch_to_bytes(card: u32, code: u16, time: NaiveDateTime, mode: u8) -> RawPunch {
        let mut res = [0; LEN];
        res[..4].copy_from_slice(&HEADER);
        res[4..6].copy_from_slice(&code.to_be_bytes());
        res[6..10].copy_from_slice(&Self::card_to_bytes(card));
        res[10..14].copy_from_slice(&Self::time_to_bytes(time));
        res[14] = mode;
        // res[15..17] is set to 0 out of 1, corresponding to the setting "send last punch"
        res[16] = 1;
        let chksum = Self::sportident_checksum(&res[2..17]).to_be_bytes();
        res[17..19].copy_from_slice(&chksum);
        res[19] = FOOTER;
        res
    }

    /// Finds a SportIdent punch record within a raw byte stream.
    ///
    /// This function searches for the `HEADER` sequence and returns the first punch found in the
    /// stream. It also returns the rest of the stream after the punch.
    ///
    /// The function is robust to some corruption, as it can handle cases where the first byte of
    /// the header is missing or the `FOOTER` is missing.
    ///
    /// # Arguments
    ///
    /// * `raw` - The byte slice to search within.
    ///
    /// # Returns
    ///
    /// An `Option` containing a tuple with:
    ///   - The `RawPunch` found.
    ///   - A slice representing the rest of the stream after the punch.
    ///
    /// Returns `None` if no punch is found.
    pub fn find_punch_data(raw: &[u8]) -> Option<(RawPunch, &[u8])> {
        let position = raw.windows(HEADER.len()).position(|w| w == HEADER);
        match position {
            Some(position) => {
                if position + LEN <= raw.len() {
                    // TODO: Also check for footer
                    Some((
                        raw[position..position + LEN].try_into().unwrap(),
                        &raw[position + LEN..],
                    ))
                } else if position + LEN == raw.len() + 1 {
                    // Add footer
                    let mut res: RawPunch = Default::default();
                    res[..LEN - 1].copy_from_slice(&raw[position..]);
                    res[LEN - 1] = FOOTER;
                    Some((res, &raw[position + LEN - 1..]))
                } else {
                    None
                }
            }
            None => {
                // Check for missing first header character
                if raw.len() >= LEN - 1 && HEADER[1..] == raw[..HEADER.len() - 1] {
                    let mut new_raw: RawPunch = Default::default();
                    new_raw[0] = HEADER[0];
                    new_raw[1..].copy_from_slice(&raw[..LEN - 1]);
                    // Solve recursively
                    Self::find_punch_data(&new_raw).map(|(punch, _)| (punch, &raw[LEN - 1..]))
                } else {
                    None
                }
            }
        }
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

    #[test]
    fn test_checksum_from_logged() {
        // Logged raw data on October 5, 2025
        let expected =
            b"\xff\x02\xd3\x0d\x00\x01\x00\x7b\xc0\xc1\x00\x9f\xa9\x20\x00\x03\x88\xf8\x93\x03";
        assert_eq!(SiPunch::sportident_checksum(&expected[2..17]), 0xf893);
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
            b"\x03\xff\x02\xd3\x0d\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\x50\xe3\x03\xff\x02";

        let punches = SiPunch::punches_from_payload::<2>(payload, time.date_naive(), time.offset());
        assert_eq!(punches.len(), 2);
        assert_eq!(*punches[0].as_ref().unwrap(), punch);
        assert_eq!(
            *punches[1].as_ref().unwrap_err(),
            Error::BufferTooSmallError
        );
    }

    #[test]
    fn test_find_punch_data() {
        let long_payload =
            b"\x03\xff\x02\xd3\x0d\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\x50\xe3\x03\xff";
        let (bytes, rest) = SiPunch::find_punch_data(long_payload).unwrap();
        assert_eq!(
            &bytes,
            b"\xff\x02\xd3\x0d\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\x50\xe3\x03"
        );
        assert_eq!(rest, b"\xff");

        let payload =
            b"\x03\xff\x02\xd3\x0d\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\x50\xe3";
        let (bytes, rest) = SiPunch::find_punch_data(payload).unwrap();
        assert_eq!(
            &bytes,
            b"\xff\x02\xd3\x0d\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\x50\xe3\x03"
        );
        assert!(rest.is_empty());

        let short_payload =
            b"\x02\xd3\x0d\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\x50\xe3\x03";
        let (bytes, rest) = SiPunch::find_punch_data(short_payload).unwrap();
        assert_eq!(
            &bytes,
            b"\xff\x02\xd3\x0d\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\x50\xe3\x03"
        );
        assert!(rest.is_empty());

        let too_short_payload =
            b"\x02\xd3\x0d\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\x50\xe3";
        let res = SiPunch::find_punch_data(too_short_payload);
        assert!(res.is_none());
    }
}
