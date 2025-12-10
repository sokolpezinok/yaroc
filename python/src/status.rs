use chrono::prelude::*;
use pyo3::{exceptions::PyValueError, prelude::*};

use yaroc_receiver::logs::CellularLogMessage as CellularLogMessageRs;
use yaroc_receiver::meshtastic::MeshtasticLog as MeshtasticLogRs;
use yaroc_receiver::state::{NodeInfo as NodeInfoRs, SignalInfo};
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
    pub rsrp_dbm: Option<i16>,
    pub snr_db: Option<f32>,
    codes: Vec<u16>,
    last_update: Option<DateTime<FixedOffset>>,
    pub last_punch: Option<DateTime<FixedOffset>>,
}

impl From<NodeInfoRs> for NodeInfo {
    fn from(node_info: NodeInfoRs) -> Self {
        let (rsrp_dbm, snr_db) = match node_info.signal_info {
            SignalInfo::Unknown => (None, None),
            SignalInfo::Cell(cell_signal_info) => (
                Some(cell_signal_info.rsrp_dbm),
                Some(cell_signal_info.snr_cb as f32 / 10.0),
            ),
            SignalInfo::Meshtastic(rssi_snr) => (Some(rssi_snr.rssi_dbm), Some(rssi_snr.snr)),
        };
        Self {
            name: node_info.name,
            rsrp_dbm,
            snr_db,
            codes: node_info.codes,
            last_update: node_info.last_update,
            last_punch: node_info.last_punch,
        }
    }
}
