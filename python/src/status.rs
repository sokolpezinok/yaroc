use chrono::prelude::*;
use pyo3::{exceptions::PyValueError, prelude::*};

use yaroc_receiver::logs::CellularLogMessage as CellularLogMessageRs;
use yaroc_receiver::meshtastic::MeshtasticLog as MeshtasticLogRs;
use yaroc_receiver::state::NodeInfo as NodeInfoRs;
use yaroc_receiver::system_info::{HostInfo as HostInfoRs, MacAddress};

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
        Ok(HostInfoRs::new(name, mac_address).into())
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
#[derive(Clone)]
pub struct CellularLog {
    inner: CellularLogMessageRs,
}

impl From<CellularLogMessageRs> for CellularLog {
    fn from(value: CellularLogMessageRs) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl CellularLog {
    pub fn __repr__(&self) -> String {
        format!("{}", self.inner)
    }

    pub fn to_proto(&self) -> Option<Vec<u8>> {
        self.inner.to_proto()
    }

    pub fn mac_address(&self) -> String {
        self.inner.mac_address().to_string()
    }
}

#[pyclass]
#[derive(Clone)]
pub struct MeshtasticLog {
    inner: MeshtasticLogRs,
}

#[pymethods]
impl MeshtasticLog {
    pub fn __repr__(&self) -> String {
        format!("{}", self.inner)
    }
}

impl From<MeshtasticLogRs> for MeshtasticLog {
    fn from(value: MeshtasticLogRs) -> Self {
        Self { inner: value }
    }
}

#[pyclass(get_all)]
#[derive(Clone)]
pub struct NodeInfo {
    pub name: String,
    pub signal_strength: String,
    codes: Vec<u16>,
    last_update: Option<DateTime<FixedOffset>>,
    pub last_punch: Option<DateTime<FixedOffset>>,
}

impl From<NodeInfoRs> for NodeInfo {
    fn from(node_info: NodeInfoRs) -> Self {
        let signal_strength = match node_info.signal_info.signal_strength() {
            yaroc_common::status::SignalStrength::Disconnected => "☆☆☆☆",
            yaroc_common::status::SignalStrength::Weak => "★☆☆☆",
            yaroc_common::status::SignalStrength::Fair => "★★☆☆",
            yaroc_common::status::SignalStrength::Good => "★★★☆",
            yaroc_common::status::SignalStrength::Excellent => "★★★★",
        }
        .to_owned();
        Self {
            name: node_info.name,
            signal_strength,
            codes: node_info.codes,
            last_update: node_info.last_update,
            last_punch: node_info.last_punch,
        }
    }
}
