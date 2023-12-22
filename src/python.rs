use geoutils::Location;
use pyo3::{exceptions::PyRuntimeError, prelude::*};

fn distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> Result<f64, String> {
    let a = Location::new(lat1, lon1);
    let b = Location::new(lat2, lon2);
    Ok(a.distance_to(&b)?.meters())
}

#[pyfunction]
pub fn geo_distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> PyResult<f64> {
    distance(lat1, lon1, lat2, lon2).map_err(|e| PyRuntimeError::new_err(e))
}

#[pymodule]
pub fn rs(_py: Python<'_>, m: &PyModule) -> PyResult<()> {
    m.add_class::<crate::punch::SiPunch>()?;
    m.add_function(wrap_pyfunction!(geo_distance, m)?)?;
    Ok(())
}
