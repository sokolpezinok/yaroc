use chrono::prelude::*;
use pyo3::{exceptions::PyValueError, prelude::*};

use yaroc_common::system_info::{HostInfo as HostInfoRs, MacAddress};

#[derive(Clone, Debug, Default, PartialEq)]
#[pyclass]
pub struct HostInfo {
    inner: HostInfoRs,
}

#[pymethods]
impl HostInfo {
    #[staticmethod]
    pub fn new(name: &str, mac_addr: &str) -> PyResult<Self> {
        let mac_address = MacAddress::try_from(mac_addr)
            .map_err(|_| PyValueError::new_err("MAC address malformatted"))?;
        Ok(HostInfoRs::new(name, mac_address)
            .map_err(|_| PyValueError::new_err("Name too long"))?
            .into())
    }

    #[getter(mac_address)]
    pub fn mac_address_str(&self) -> String {
        self.inner.mac_address.to_string()
    }
}

impl HostInfo {
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    pub fn mac_address(&self) -> &MacAddress {
        &self.inner.mac_address
    }
}

impl From<HostInfoRs> for HostInfo {
    fn from(host_info: HostInfoRs) -> Self {
        Self { inner: host_info }
    }
}

impl From<HostInfo> for HostInfoRs {
    fn from(host_info: HostInfo) -> Self {
        host_info.inner
    }
}

#[pyclass]
pub struct NodeInfo {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub rssi_dbm: Option<i16>,
    #[pyo3(get)]
    pub snr_db: Option<f32>,
    #[pyo3(get)]
    cellid: Option<u32>,
    #[pyo3(get)]
    codes: Vec<u16>,
    #[pyo3(get)]
    last_update: Option<DateTime<FixedOffset>>,
    #[pyo3(get)]
    pub last_punch: Option<DateTime<FixedOffset>>,
}

impl From<yaroc_common::receive::state::NodeInfo> for NodeInfo {
    fn from(value: yaroc_common::receive::state::NodeInfo) -> Self {
        Self {
            name: value.name,
            rssi_dbm: value.rssi_dbm,
            snr_db: value.snr_db,
            cellid: value.cellid,
            codes: value.codes,
            last_update: value.last_update,
            last_punch: value.last_punch,
        }
    }
}
