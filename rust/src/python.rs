use pyo3::prelude::*;

#[pyfunction]
pub fn sportident_checksum(message: &[u8]) -> Vec<u8> {
    let chksum = crate::checksum::sportident_checksum(message);
    chksum.to_le_bytes().into_iter().collect()
}

#[pymodule]
pub fn yaroc_rs(_py: Python<'_>, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(sportident_checksum, m)?)?;
    Ok(())
}
