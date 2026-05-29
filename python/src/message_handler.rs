use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use yaroc_receiver::usb_serial_manager::UsbSerialManager as UsbSerialManagerRs;

use yaroc_receiver::logs::{CellularLogMessage, SiPunchLog as SiPunchLogRs};
use yaroc_receiver::message_handler::MessageHandler as MessageHandlerRs;
use yaroc_receiver::mqtt::MqttConfig as MqttConfigRs;
use yaroc_receiver::state::Event as EventRs;
use yaroc_receiver::system_info::MacAddress;

use crate::punch::{SiPunch, SiPunchLog};
use crate::status::{CellularLog, MeshtasticLog, NodeInfo};

/// Events that can be processed by the Python application.
#[pyclass]
pub enum Event {
    CellularLog(CellularLog),
    SiPunchLogs(Vec<SiPunchLog>),
    SiPunch(SiPunch),
    MeshtasticLog(MeshtasticLog),
    NodeInfos(Vec<NodeInfo>),
    DeviceEvent { added: bool, device: String },
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

/// Configuration for the MQTT client.
#[pyclass(get_all, set_all, from_py_object)]
#[derive(Clone)]
pub struct MqttConfig {
    url: String,
    port: u16,
    credentials: Option<(String, String)>,
    keep_alive: Duration,
    meshtastic_channel: Option<String>,
}

#[pymethods]
impl MqttConfig {
    /// Creates a default MQTT configuration.
    #[new]
    pub fn new() -> Self {
        Self::default()
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
            credentials: config.credentials,
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
            credentials: config.credentials,
            keep_alive: config.keep_alive,
            meshtastic_channel: config.meshtastic_channel,
        }
    }
}

/// Manages Meshtastic devices connected via serial ports.
#[pyclass]
pub struct UsbSerialManager {
    inner: Arc<Mutex<UsbSerialManagerRs>>,
}

#[pymethods]
impl UsbSerialManager {
    /// Asynchronously runs a background loop to automatically monitor USB hotplug events
    /// and add/remove Meshtastic devices accordingly.
    pub fn r#loop<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py::<_, ()>(py, async move {
            let mut manager = inner.lock().await;
            manager
                .monitor_usb_devices()
                .await
                .map_err(|err| PyRuntimeError::new_err(format!("USB monitor error: {err}")))
        })
    }
}

/// Central handler for aggregating and dispatching messages.
///
/// Handles communication between various sources (Serial, Meshtastic) and sinks (MQTT).
#[pyclass]
pub struct MessageHandler {
    inner: Arc<Mutex<MessageHandlerRs>>,
}

#[pymethods]
impl MessageHandler {
    /// Creates a new MessageHandler.
    ///
    /// # Arguments
    ///
    /// * `dns` - A list of (mac_address, name) tuples for resolving node names.
    /// * `mqtt_config` - Optional MQTT configuration.
    /// * `node_info_interval` - Interval for sending node info messages.
    #[new]
    #[pyo3(signature = (dns, mqtt_config=None, node_info_interval = Duration::from_secs(60)))]
    pub fn new(
        dns: Vec<(String, String)>,
        mqtt_config: Option<MqttConfig>,
        node_info_interval: Duration,
    ) -> PyResult<Self> {
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
            mqtt_config.map(|config| config.into()).into_iter().collect(),
            node_info_interval,
        )));
        Ok(Self { inner })
    }

    /// Returns the handler for serial devices.
    #[pyo3(signature = (enable_meshtastic=true, enable_sportident=true))]
    pub fn usb_serial_manager(
        &self,
        enable_meshtastic: bool,
        enable_sportident: bool,
    ) -> PyResult<UsbSerialManager> {
        let handler = self.get_inner()?.usb_serial_manager(enable_meshtastic, enable_sportident);
        Ok(UsbSerialManager {
            inner: Arc::new(Mutex::new(handler)),
        })
    }

    /// Waits for the next event from the message handler.
    pub fn next_event<'a>(&'a self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let handler = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py::<_, Event>(py, async move {
            let mut handler = handler.lock().await;
            let message =
                handler.next_event().await.map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            match message {
                EventRs::CellularLog(cellular_log) => Ok(cellular_log.into()),
                EventRs::SiPunches(si_punch_logs) => Ok(si_punch_logs.into()),
                EventRs::SiPunch(si_punch) => Ok(Event::SiPunch(si_punch.into())),
                EventRs::MeshtasticLog(meshtastic_log) => {
                    Ok(Event::MeshtasticLog(meshtastic_log.into()))
                }
                EventRs::NodeInfos(node_infos) => Ok(Event::NodeInfos(
                    node_infos.into_iter().map(|info| info.into()).collect(),
                )),
                EventRs::DeviceEvent { added, device } => Ok(Event::DeviceEvent { added, device }),
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
