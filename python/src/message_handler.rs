use std::sync::Arc;
use std::time::Duration;

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;

use tokio::sync::Mutex;
use yaroc_common::logs::CellularLogMessage;
use yaroc_common::punch::SiPunchLog as SiPunchLogRs;
use yaroc_common::receive::message_handler::{
    Message as MessageRs, MessageHandler as MessageHandlerRs,
};
use yaroc_common::receive::mqtt::MqttConfig as MqttConfigRs;
use yaroc_common::system_info::MacAddress;

use crate::punch::SiPunchLog;
use crate::status::{CellularLog, NodeInfo};

#[pyclass]
pub enum Message {
    CellularLog(CellularLog),
    SiPunchLogs(Vec<SiPunchLog>),
    MeshtasticLog(),
}

impl From<Vec<SiPunchLogRs>> for Message {
    fn from(logs: Vec<SiPunchLogRs>) -> Self {
        Self::SiPunchLogs(logs.into_iter().map(SiPunchLog::from).collect())
    }
}

impl From<CellularLogMessage> for Message {
    fn from(log: CellularLogMessage) -> Self {
        Self::CellularLog(log.into())
    }
}

#[pyclass]
#[derive(Clone)]
pub struct MqttConfig {
    #[pyo3(get, set)]
    url: String,
    #[pyo3(get, set)]
    port: u16,
    #[pyo3(get, set)]
    keep_alive: Duration,
    #[pyo3(get, set)]
    meshtastic_channel: Option<String>,
}

#[pymethods]
impl MqttConfig {
    #[new]
    pub fn new() -> Self {
        MqttConfigRs::default().into()
    }
}

impl Default for MqttConfig {
    fn default() -> Self {
        MqttConfigRs::default().into()
    }
}

impl From<MqttConfigRs> for MqttConfig {
    fn from(config: MqttConfigRs) -> Self {
        Self {
            url: config.url,
            port: config.port,
            keep_alive: config.keep_alive,
            meshtastic_channel: config.meshtastic_channel,
        }
    }
}

impl From<MqttConfig> for MqttConfigRs {
    fn from(config: MqttConfig) -> Self {
        Self {
            url: config.url,
            port: config.port,
            keep_alive: config.keep_alive,
            meshtastic_channel: config.meshtastic_channel,
        }
    }
}

#[pyclass]
pub struct MessageHandler {
    inner: Arc<Mutex<MessageHandlerRs>>,
}

#[pymethods]
impl MessageHandler {
    #[new]
    #[pyo3(signature = (dns, mqtt_config=None))]
    pub fn new_py(dns: Vec<(String, String)>, mqtt_config: Option<MqttConfig>) -> PyResult<Self> {
        let dns: PyResult<Vec<(String, MacAddress)>> = dns
            .into_iter()
            .map(|(mac, name)| {
                Ok((
                    name,
                    MacAddress::try_from(mac.as_str()).map_err(|_| {
                        PyValueError::new_err(format!("Wrong MAC address format: {mac}"))
                    })?,
                ))
            })
            .collect();
        let inner = Arc::new(Mutex::new(MessageHandlerRs::new(
            dns?,
            mqtt_config.map(|config| config.into()),
        )));
        Ok(Self { inner })
    }

    pub fn node_infos(&mut self) -> PyResult<Vec<NodeInfo>> {
        Ok(self.get_inner()?.node_infos().into_iter().map(|n| n.into()).collect())
    }

    pub fn next_message<'a>(&'a self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let handler = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py::<_, Message>(py, async move {
            let mut handler = handler.lock().await;
            let message = handler
                .next_message()
                .await
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            match message {
                MessageRs::CellularLog(cellular_log) => Ok(cellular_log.into()),
                MessageRs::SiPunches(si_punch_logs) => Ok(si_punch_logs.into()),
                MessageRs::MeshtasticLog => Ok(Message::MeshtasticLog()),
            }
        })
    }
}

impl MessageHandler {
    fn get_inner(&mut self) -> PyResult<tokio::sync::MutexGuard<'_, MessageHandlerRs>> {
        self.inner
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("Failed to lock message handler".to_owned()))
    }
}
