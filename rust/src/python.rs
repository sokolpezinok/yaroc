use std::borrow::Cow;

use chrono::{DateTime, FixedOffset, NaiveDateTime};
use pyo3::prelude::*;

#[pyfunction]
pub fn sportident_checksum(message: &[u8]) -> Vec<u8> {
    let chksum = crate::punch::sportident_checksum(message);
    chksum.to_le_bytes().into_iter().collect()
}

fn timestamp_to_datetime(timestamp: f64, tz_offset_secs: i32) -> Option<DateTime<FixedOffset>> {
    let secs = timestamp as i64;
    let nanos = ((timestamp - secs as f64) * 1e9) as u32;
    let tz = FixedOffset::east_opt(tz_offset_secs).unwrap();
    NaiveDateTime::from_timestamp_opt(secs, nanos)
        .map(|time| DateTime::from_naive_utc_and_offset(time, tz))
}

#[pyfunction]
pub fn punch_to_bytes<'a>(
    card: u32,
    code: u16,
    timestamp: f64,
    tz_offset_secs: f64,
    mode: u8,
) -> Cow<'a, [u8]> {
    match timestamp_to_datetime(timestamp, tz_offset_secs.round() as i32) {
        None => [0; 20].into_iter().collect(),
        Some(time) => crate::punch::punch_to_bytes(code, time, card, mode)
            .into_iter()
            .collect(),
    }
}

#[pymodule]
pub fn yaroc_rs(_py: Python<'_>, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(sportident_checksum, m)?)?;
    m.add_function(wrap_pyfunction!(punch_to_bytes, m)?)?;
    Ok(())
}

mod test_conversion {
    use super::timestamp_to_datetime;

    #[test]
    fn test_timestamp() {
        let time = timestamp_to_datetime(24. * 60. * 60. + 123., 3600).unwrap();
        let str = format!("{time}");
        assert_eq!(str, "1970-01-02 01:02:03 +01:00");
    }
}
