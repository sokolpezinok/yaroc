use pyo3::prelude::*;

#[pymodule]
pub fn rs(_py: Python<'_>, m: &PyModule) -> PyResult<()> {
    m.add_class::<crate::punch::SiPunch>()?;
    Ok(())
}
