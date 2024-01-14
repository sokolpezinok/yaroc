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

#[pyclass]
pub enum RaspberryModel {
    Unknown = 0,
    V1A = 1,
    V1B = 2,
    V1Ap = 3,
    V1Bp = 4,
    V2A = 5,
    V2B = 6,
    V3A = 7,
    V3B = 8,
    V3Ap = 9,
    V3Bp = 10,
    V4A = 11,
    V4B = 12,
    V5A = 13,
    V5B = 14,
    VZero = 15,
    VZeroW = 16,
}

#[pymethods]
impl RaspberryModel {
    #[staticmethod]
    pub fn from_string(model: &str) -> RaspberryModel {
        let re = regex::Regex::new("Raspberry Pi ([1-5]) Model ([AB])").unwrap();
        if let Some(captures) = re.captures(model) {
            let captures_vec: Vec<_> = captures
                .iter()
                .skip(1)
                .filter_map(|m| m.map(|matc| matc.as_str()))
                .collect();
            match captures_vec.as_slice() {
                ["1", "A"] => RaspberryModel::V1A,
                ["1", "B"] => RaspberryModel::V1B,
                ["2", "A"] => RaspberryModel::V2A,
                ["2", "B"] => RaspberryModel::V2B,
                ["3", "A"] => RaspberryModel::V3A,
                ["3", "B"] => RaspberryModel::V3B,
                ["4", "A"] => RaspberryModel::V4A,
                ["4", "B"] => RaspberryModel::V4B,
                ["5", "A"] => RaspberryModel::V5A,
                ["5", "B"] => RaspberryModel::V5B,
                _ => RaspberryModel::Unknown,
            }
        } else {
            RaspberryModel::VZero
        }
    }
}

#[pymodule]
pub fn rs(_py: Python<'_>, m: &PyModule) -> PyResult<()> {
    m.add_class::<crate::punch::SiPunch>()?;
    m.add_function(wrap_pyfunction!(geo_distance, m)?)?;
    m.add_class::<RaspberryModel>()?;
    Ok(())
}
