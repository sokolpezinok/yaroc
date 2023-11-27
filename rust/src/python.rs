use std::borrow::Cow;

use chrono::{FixedOffset, NaiveDateTime};
use pyo3::prelude::*;

#[pyfunction]
pub fn sportident_checksum(message: &[u8]) -> Vec<u8> {
    let chksum = crate::punch::sportident_checksum(message);
    chksum.to_le_bytes().into_iter().collect()
}

#[pyfunction]
pub fn punch_to_bytes<'a>(
    card: u32,
    code: u16,
    timestamp: f64,
    tz_offset_secs: f64,
    mode: u8,
) -> Cow<'a, [u8]> {
    let secs = timestamp as i64;
    let nanos = ((timestamp - secs as f64) * 1e9) as u32;
    let tz = FixedOffset::east_opt(tz_offset_secs.round() as i32).unwrap();
    match NaiveDateTime::from_timestamp_opt(secs, nanos) {
        None => [0; 20].into_iter().collect(),
        Some(time) => {
            let time_tz = time.and_local_timezone(tz).unwrap();
            crate::punch::punch_to_bytes(code, time_tz, card, mode)
                .into_iter()
                .collect()
        }
    }
}

#[pymodule]
pub fn yaroc_rs(_py: Python<'_>, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(sportident_checksum, m)?)?;
    m.add_function(wrap_pyfunction!(punch_to_bytes, m)?)?;
    Ok(())
}
