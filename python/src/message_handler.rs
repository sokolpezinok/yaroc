use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use yaroc_receiver::logs::{CellularLogMessage, SiPunchLog as SiPunchLogRs};
use yaroc_receiver::meshtastic_serial::MshDevHandler as MshDevNotifierRs;
use yaroc_receiver::message_handler::MessageHandler as MessageHandlerRs;
use yaroc_receiver::mqtt::MqttConfig as MqttConfigRs;
use yaroc_receiver::state::Event as EventRs;
use yaroc_receiver::system_info::MacAddress;

use crate::punch::SiPunchLog;
use crate::status::{CellularLog, MeshtasticLog, NodeInfo};

#[pyclass]
pub enum Event {
    CellularLog(CellularLog),
    SiPunchLogs(Vec<SiPunchLog>),
    MeshtasticLog(MeshtasticLog),
    NodeInfos(Vec<NodeInfo>),
}

impl From<Vec<SiPunchLogRs>> for Event {
    fn from(logs: Vec<SiPunchLogRs>) -> Self {
        Self::SiPunchLogs(logs.into_iter().map(SiPunchLog::from).collect())
    }
}

impl From<CellularLogMessage> for Event {
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
pub struct MshDevHandler {
    inner: Arc<Mutex<MshDevNotifierRs>>,
}

#[pymethods]
impl MshDevHandler {
    pub fn add_device<'a>(
        &mut self,
        py: Python<'a>,
        port: String,
        device_node: String,
    ) -> PyResult<Bound<'a, PyAny>> {
        let notifier = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            notifier.lock().await.add_device(&port, &device_node).await;
            Ok(())
        })
    }

    pub fn remove_device(&mut self, device_node: String) -> PyResult<()> {
        self.inner
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("Failed to device notifier".to_owned()))?
            .remove_device(device_node);
        Ok(())
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
    pub fn new(dns: Vec<(String, String)>, mqtt_config: Option<MqttConfig>) -> PyResult<Self> {
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

    pub fn msh_dev_handler(&self) -> PyResult<MshDevHandler> {
        let handler = self.get_inner()?.meshtastic_device_handler();
        Ok(MshDevHandler {
            inner: Arc::new(Mutex::new(handler)),
        })
    }

    pub fn next_event<'a>(&'a self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let handler = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py::<_, Event>(py, async move {
            let mut handler = handler.lock().await;
            let message =
                handler.next_event().await.map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            match message {
                EventRs::CellularLog(cellular_log) => Ok(cellular_log.into()),
                EventRs::SiPunches(si_punch_logs) => Ok(si_punch_logs.into()),
                EventRs::MeshtasticLog(meshtastic_log) => {
                    Ok(Event::MeshtasticLog(meshtastic_log.into()))
                }
                EventRs::NodeInfos(node_infos) => Ok(Event::NodeInfos(
                    node_infos.into_iter().map(|info| info.into()).collect(),
                )),
            }
        })
    }
}

impl MessageHandler {
    fn get_inner(&self) -> PyResult<tokio::sync::MutexGuard<'_, MessageHandlerRs>> {
        self.inner
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("Failed to lock message handler".to_owned()))
    }
}
