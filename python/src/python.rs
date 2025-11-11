use chrono::Local;
use log::LevelFilter;
use pyo3::prelude::*;
use pyo3_log::{Caching, Logger};

#[pyclass(eq, eq_int)]
#[derive(PartialEq)]
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
            let captures_vec: Vec<_> =
                captures.iter().skip(1).filter_map(|m| m.map(|matc| matc.as_str())).collect();
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

#[pyfunction]
pub fn current_timestamp_millis() -> i64 {
    Local::now().fixed_offset().timestamp_millis()
}

#[pymodule]
pub fn rs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<crate::punch::SiPunch>()?;
    m.add_class::<crate::punch::SiPunchLog>()?;
    m.add_class::<crate::status::HostInfo>()?;
    m.add_class::<RaspberryModel>()?;
    m.add_function(wrap_pyfunction!(current_timestamp_millis, m)?)?;

    m.add_class::<crate::message_handler::Event>()?;
    m.add_class::<crate::message_handler::MessageHandler>()?;
    m.add_class::<crate::message_handler::MqttConfig>()?;
    m.add_class::<crate::message_handler::MshDevHandler>()?;
    m.add_class::<crate::serial_client::SerialClient>()?;
    m.add_class::<crate::si_uart::SiUartHandler>()?;
    m.add_class::<crate::status::CellularLog>()?;
    m.add_class::<crate::status::MeshtasticLog>()?;
    m.add_class::<crate::status::NodeInfo>()?;

    let _ = Logger::new(m.py(), Caching::LoggersAndLevels)?
        .filter(LevelFilter::Trace)
        // TODO: remove `.filter_target` once https://github.com/meshtastic/rust/issues/27 is fixed
        .filter_target(
            "meshtastic::connections::stream_buffer".to_owned(),
            LevelFilter::Off,
        )
        .install()
        .expect("Someone installed a logger before us");

    Ok(())
}
