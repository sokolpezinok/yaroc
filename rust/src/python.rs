use pyo3::prelude::*;

#[pyfunction]
pub fn sportident_checksum(message: &[u8]) -> Vec<u8> {
    let chksum = crate::checksum::sportident_checksum(message);
    chksum.to_le_bytes().into_iter().collect()
}

/// A Python module implemented in Rust. The name of this function must match
/// the `lib.name` setting in the `Cargo.toml`, else Python will not be able to
/// import the module.
#[pymodule]
pub fn yaroc(_py: Python<'_>, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(sportident_checksum, m)?)?;
    Ok(())
}
