use chrono::NaiveDateTime;
use pyo3::prelude::*;

#[pyfunction]
pub fn sportident_checksum(message: &[u8]) -> Vec<u8> {
    let chksum = crate::punch::sportident_checksum(message);
    chksum.to_le_bytes().into_iter().collect()
}

#[pyfunction]
pub fn punch_to_bytes(card: u32, code: u16, timestamp: f64, mode: u8) -> [u8; 20] {
    let secs = timestamp as i64;
    let nanos = ((timestamp - secs as f64) * 1e9) as u32;
    match NaiveDateTime::from_timestamp_opt(secs, nanos) {
        None => [0; 20],
        Some(time) => crate::punch::punch_to_bytes(code, time, card, mode),
    }
}

#[pymodule]
pub fn yaroc_rs(_py: Python<'_>, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(sportident_checksum, m)?)?;
    m.add_function(wrap_pyfunction!(punch_to_bytes, m)?)?;
    Ok(())
}
