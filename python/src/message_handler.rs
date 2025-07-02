use chrono::DateTime;
use chrono::prelude::*;
use log::info;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;

use yaroc_common::error::Error;
use yaroc_common::receive::message_handler::MessageHandler as MessageHandlerRs;
use yaroc_common::system_info::MacAddress;

use crate::punch::SiPunchLog;
use crate::status::NodeInfo;

#[pyclass]
pub struct MessageHandler {
    inner: MessageHandlerRs,
}

#[pymethods]
impl MessageHandler {
    #[new]
    pub fn new_py(dns: Vec<(String, String)>) -> PyResult<Self> {
        MessageHandlerRs::new(dns)
            .map_err(|err| match err {
                Error::ParseError | Error::ValueError => {
                    PyValueError::new_err("Wrong MAC address format")
                }
                _ => PyRuntimeError::new_err("Unknown error"),
            })
            .map(|inner| Self { inner })
    }

    pub fn meshtastic_serial_service_envelope(
        &mut self,
        payload: &[u8],
    ) -> PyResult<Vec<SiPunchLog>> {
        self.inner
            .msh_serial_service_envelope(payload)
            .map(|punches| punches.into_iter().map(SiPunchLog::from).collect())
            .map_err(PyErr::from)
    }

    pub fn meshtastic_serial_mesh_packet(&mut self, payload: &[u8]) -> PyResult<Vec<SiPunchLog>> {
        self.inner
            .msh_serial_mesh_packet(payload)
            .map(|punches| punches.into_iter().map(SiPunchLog::from).collect())
            .map_err(PyErr::from)
    }

    #[pyo3(signature = (payload, now, recv_mac_address=None))]
    pub fn meshtastic_status_service_envelope(
        &mut self,
        payload: &[u8],
        now: DateTime<FixedOffset>,
        recv_mac_address: Option<u32>,
    ) {
        self.inner.msh_status_service_envelope(payload, now, recv_mac_address);
    }

    #[pyo3(signature = (payload, now, recv_mac_address=None))]
    pub fn meshtastic_status_mesh_packet(
        &mut self,
        payload: &[u8],
        now: DateTime<FixedOffset>,
        recv_mac_address: Option<u32>,
    ) {
        self.inner.msh_status_mesh_packet(payload, now, recv_mac_address);
    }

    pub fn node_infos(&self) -> Vec<NodeInfo> {
        self.inner.node_infos().into_iter().map(|n| n.into()).collect()
    }

    pub fn punches(&mut self, payload: &[u8], mac_addr: u64) -> PyResult<Vec<SiPunchLog>> {
        let mac_addr = MacAddress::Full(mac_addr);
        self.inner
            .punches(payload, mac_addr)
            .map(|punches| punches.into_iter().map(SiPunchLog::from).collect())
            .map_err(|err| PyValueError::new_err(format!("{err}")))
    }

    #[pyo3(name = "status_update")]
    pub fn status_update_py(&mut self, payload: &[u8], mac_addr: u64) -> PyResult<()> {
        let mac_addr = MacAddress::Full(mac_addr);
        let log_message = self.inner.status_update(payload, mac_addr).map_err(|e| match e {
            Error::ParseError => PyValueError::new_err("Status proto decoding error"),
            Error::FormatError => PyValueError::new_err("Missing time in status proto"),
            _ => PyValueError::new_err(format!("{}", e)),
        })?;
        info!("{log_message}");
        Ok(())
    }
}
