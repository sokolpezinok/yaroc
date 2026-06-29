use chrono::prelude::*;
use meshtastic::Message as _;
use meshtastic::protobufs::ServiceEnvelope;
use pyo3::{exceptions::PyValueError, prelude::*};

use yaroc_common::status::SignalStrength;
use yaroc_receiver::logs::CellularLogMessage as CellularLogMessageRs;
use yaroc_receiver::meshtastic::{MESHTASTIC_MQTT_PREFIX, MeshtasticLog as MeshtasticLogRs};
use yaroc_receiver::state::NodeInfo as NodeInfoRs;
use yaroc_receiver::system_info::{HostInfo as HostInfoRs, MacAddress};

#[derive(Clone, Debug, Default, PartialEq)]
#[pyclass(from_py_object)]
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

#[pyclass(from_py_object)]
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

#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct MeshtasticLog {
    inner: MeshtasticLogRs,
    service_envelope: Box<ServiceEnvelope>,
}

#[pymethods]
impl MeshtasticLog {
    pub fn __repr__(&self) -> String {
        format!("{}", self.inner)
    }

    #[getter]
    pub fn service_envelope(&self) -> Vec<u8> {
        self.service_envelope.encode_to_vec()
    }

    #[getter]
    pub fn mqtt_topic(&self) -> String {
        format!(
            "{}{}/{}",
            MESHTASTIC_MQTT_PREFIX,
            self.service_envelope.channel_id,
            self.service_envelope.gateway_id
        )
    }
}

impl MeshtasticLog {
    pub fn new(inner: MeshtasticLogRs, service_envelope: Box<ServiceEnvelope>) -> Self {
        Self {
            inner,
            service_envelope,
        }
    }
}

#[pyclass(get_all, from_py_object)]
#[derive(Clone)]
pub struct NodeInfo {
    pub name: String,
    pub signal_strength: String,
    pub battery_percentage: Option<u8>,
    codes: Vec<u16>,
    last_update: Option<DateTime<FixedOffset>>,
    pub last_punch: Option<DateTime<FixedOffset>>,
}

impl From<NodeInfoRs> for NodeInfo {
    fn from(node_info: NodeInfoRs) -> Self {
        let signal_strength = match node_info.signal_info.signal_strength() {
            SignalStrength::Disconnected => "____",
            SignalStrength::Weak => "▂___",
            SignalStrength::Fair => "▂▄__",
            SignalStrength::Good => "▂▄▆_",
            SignalStrength::Excellent => "▂▄▆█",
        }
        .to_owned();
        Self {
            name: node_info.name,
            signal_strength,
            battery_percentage: node_info.battery_percentage,
            codes: node_info.codes,
            last_update: node_info.last_update,
            last_punch: node_info.last_punch,
        }
    }
}
