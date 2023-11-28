use std::borrow::Cow;

use chrono::{DateTime, FixedOffset};
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
    time: DateTime<FixedOffset>,
    mode: u8,
) -> Cow<'a, [u8]> {
    crate::punch::punch_to_bytes(code, time, card, mode)
        .into_iter()
        .collect()
}

#[pymodule]
pub fn yaroc_rs(_py: Python<'_>, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(sportident_checksum, m)?)?;
    m.add_function(wrap_pyfunction!(punch_to_bytes, m)?)?;
    m.add_class::<crate::punch::SiPunch>()?;
    Ok(())
}
