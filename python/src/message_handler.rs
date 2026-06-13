use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use yaroc_receiver::usb_serial_manager::UsbSerialManager as UsbSerialManagerRs;

use yaroc_receiver::logs::{CellularLogMessage, SiPunchLog as SiPunchLogRs};
use yaroc_receiver::message_handler::{
    MessageHandler as MessageHandlerRs, MessageHandlerBuilder, SportIdentConfig, UsbSerialConfig,
};
use yaroc_receiver::mqtt::MqttConfig as MqttConfigRs;
use yaroc_receiver::state::Event as EventRs;
use yaroc_receiver::system_info::MacAddress;

use crate::punch::{SiPunch, SiPunchLog};
use crate::serial_client::PyUsbSerialFactory;
use crate::status::{CellularLog, MeshtasticLog, NodeInfo};

/// Events that can be processed by the Python application.
#[pyclass]
pub enum Event {
    CellularLog(CellularLog),
    SiPunchLogs(Vec<SiPunchLog>),
    SiPunch(SiPunch),
    MeshtasticLog(MeshtasticLog),
    NodeInfos(Vec<NodeInfo>),
    DeviceEvnt { added: bool, device: String },
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
    pub async fn r#loop(&self) -> PyResult<()> {
        let inner = self.inner.clone();
        crate::python::run_on_tokio(async move {
            let mut manager = inner.lock().await;
            manager
                .monitor_usb_devices()
                .await
                .map_err(|err| PyRuntimeError::new_err(format!("USB monitor error: {err}")))
        })
        .await
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
    /// * `mqtt_configs` - A list of MQTT configurations.
    /// * `node_info_interval` - Interval for sending node info messages.
    /// * `meshtastic_timeout` - Timeout for Meshtastic nodes.
    /// * `enable_meshtastic` - Whether to enable Meshtastic support.
    /// * `enable_sportident` - Whether to enable SportIdent support.
    /// * `sportident_factory` - Optional factory for creating SportIdent serial connections.
    #[staticmethod]
    #[pyo3(signature = (dns, mqtt_configs=Vec::new(), node_info_interval = Duration::from_secs(60), meshtastic_timeout = Duration::from_secs(600), enable_meshtastic=false, enable_sportident=false, sportident_factory=None))]
    pub fn new(
        dns: Vec<(String, String)>,
        mqtt_configs: Vec<MqttConfig>,
        node_info_interval: Duration,
        meshtastic_timeout: Duration,
        enable_meshtastic: bool,
        enable_sportident: bool,
        sportident_factory: Option<Bound<'_, PyUsbSerialFactory>>,
    ) -> PyResult<(Self, UsbSerialManager)> {
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

        let sportident = if let Some(factory_bound) = sportident_factory {
            let factory = factory_bound.borrow().clone();
            SportIdentConfig::Active(Box::new(factory))
        } else if enable_sportident {
            SportIdentConfig::Passive
        } else {
            SportIdentConfig::None
        };
        let usb_serial_config = UsbSerialConfig {
            enable_meshtastic,
            sportident,
        };

        let (message_handler_rs, usb_serial_manager_rs) = MessageHandlerBuilder::new()
            .with_dns(dns?)
            .with_mqtt_configs(mqtt_configs.into_iter().map(|config| config.into()).collect())
            .with_node_infos_interval(node_info_interval)
            .with_meshtastic_timeout(meshtastic_timeout)
            .with_usb_serial_config(usb_serial_config)
            .build();
        let inner = Arc::new(Mutex::new(message_handler_rs));
        Ok((
            Self { inner },
            UsbSerialManager {
                inner: Arc::new(Mutex::new(usb_serial_manager_rs)),
            },
        ))
    }

    /// Waits for the next event from the message handler.
    pub async fn next_event(&self) -> PyResult<Event> {
        let inner = self.inner.clone();
        crate::python::run_on_tokio(async move {
            let mut handler = inner.lock().await;
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
                EventRs::DeviceEvent { added, device } => Ok(Event::DeviceEvnt { added, device }),
            }
        })
        .await
    }
}
