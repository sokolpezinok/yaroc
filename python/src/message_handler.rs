use meshtastic::Message as _;
use meshtastic::protobufs::ServiceEnvelope;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use yaroc_receiver::logs::{CellularLogMessage, SiPunchLog as SiPunchLogRs};
use yaroc_receiver::meshtastic::MESHTASTIC_MQTT_PREFIX;
use yaroc_receiver::message_handler::{
    MessageHandler as MessageHandlerRs, MessageHandlerBuilder as MessageHandlerBuilderRs,
    SportIdentConfig, UsbSerialConfig,
};
use yaroc_receiver::mqtt::MqttConfig as MqttConfigRs;
use yaroc_receiver::state::Event as EventRs;
use yaroc_receiver::system_info::MacAddress;

use crate::punch::{SiPunch, SiPunchLog};
use crate::serial_client::PyUsbSerialFactory;
use crate::status::{CellularLog, MeshtasticLog, NodeInfo};

#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct MeshtasticPunches {
    #[pyo3(get)]
    pub punch_logs: Vec<SiPunchLog>,
    service_envelope: ServiceEnvelope,
}

#[pymethods]
impl MeshtasticPunches {
    #[getter]
    pub fn service_envelope(&self) -> Vec<u8> {
        self.service_envelope.encode_to_vec()
    }

    #[getter]
    pub fn mqtt_topic(&self) -> String {
        format!(
            "{}serial/{}",
            MESHTASTIC_MQTT_PREFIX, self.service_envelope.gateway_id
        )
    }
}

/// Events that can be processed by the Python application.
#[pyclass]
pub enum Event {
    CellularLog(CellularLog),
    SiPunchLogs(Vec<SiPunchLog>),
    MeshtasticPunches(MeshtasticPunches),
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

/// Central handler for aggregating and dispatching messages.
///
/// Handles communication between various sources (Serial, Meshtastic) and sinks (MQTT).
#[pyclass]
pub struct MessageHandler {
    inner: Arc<Mutex<MessageHandlerRs>>,
}

#[pymethods]
impl MessageHandler {
    /// Waits for the next event from the message handler.
    pub async fn next_event(&self) -> PyResult<Event> {
        let inner = self.inner.clone();
        crate::python::run_on_tokio(async move {
            let mut handler = inner.lock().await;
            let message =
                handler.next_event().await.map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            let event = match message {
                EventRs::CellularLog(cellular_log) => cellular_log.into(),
                EventRs::SiPunches(si_punch_logs) => si_punch_logs.into(),
                EventRs::SiPunchesMeshtastic(si_punch_logs, service_envelope) => {
                    Event::MeshtasticPunches(MeshtasticPunches {
                        punch_logs: si_punch_logs.into_iter().map(SiPunchLog::from).collect(),
                        service_envelope,
                    })
                }
                EventRs::SiPunch(si_punch) => Event::SiPunch(si_punch.into()),
                EventRs::MeshtasticLog(meshtastic_log, service_envelope) => {
                    Event::MeshtasticLog(MeshtasticLog::new(meshtastic_log, service_envelope))
                }
                EventRs::NodeInfos(node_infos) => {
                    Event::NodeInfos(node_infos.into_iter().map(From::from).collect())
                }
                EventRs::DeviceEvent { added, device } => Event::DeviceEvnt { added, device },
            };
            Ok(event)
        })
        .await
    }
}

/// A builder to construct `MessageHandler` and `UsbSerialManager`.
#[pyclass]
pub struct MessageHandlerBuilder {
    dns: Vec<(String, MacAddress)>,
    mqtt_configs: Vec<MqttConfig>,
    node_info_interval: Duration,
    meshtastic_timeout: Duration,
    enable_meshtastic: bool,
    enable_sportident: bool,
    sportident_factory: Option<Py<PyUsbSerialFactory>>,
    meshtastic_tcp: Option<String>,
    fake_punch_interval: Option<Duration>,
}

#[pymethods]
impl MessageHandlerBuilder {
    #[new]
    pub fn new() -> Self {
        Self {
            dns: Vec::new(),
            mqtt_configs: Vec::new(),
            node_info_interval: Duration::from_secs(60),
            meshtastic_timeout: Duration::from_secs(600),
            enable_meshtastic: false,
            enable_sportident: false,
            sportident_factory: None,
            meshtastic_tcp: None,
            fake_punch_interval: None,
        }
    }

    pub fn with_dns<'py>(
        mut self_: PyRefMut<'py, Self>,
        dns: Vec<(String, String)>,
    ) -> PyResult<PyRefMut<'py, Self>> {
        self_.dns = dns
            .into_iter()
            .map(|(mac, name)| -> PyResult<(String, MacAddress)> {
                Ok((
                    name,
                    MacAddress::try_from(mac.as_str()).map_err(|_| {
                        PyValueError::new_err(format!("Wrong MAC address format: {mac}"))
                    })?,
                ))
            })
            .collect::<PyResult<Vec<_>>>()?;
        Ok(self_)
    }

    pub fn with_mqtt_configs<'py>(
        mut self_: PyRefMut<'py, Self>,
        mqtt_configs: Vec<MqttConfig>,
    ) -> PyRefMut<'py, Self> {
        self_.mqtt_configs = mqtt_configs;
        self_
    }

    pub fn with_node_info_interval<'py>(
        mut self_: PyRefMut<'py, Self>,
        interval: Duration,
    ) -> PyRefMut<'py, Self> {
        self_.node_info_interval = interval;
        self_
    }

    pub fn with_meshtastic_timeout<'py>(
        mut self_: PyRefMut<'py, Self>,
        timeout: Duration,
    ) -> PyRefMut<'py, Self> {
        self_.meshtastic_timeout = timeout;
        self_
    }

    pub fn with_meshtastic<'py>(
        mut self_: PyRefMut<'py, Self>,
        enable: bool,
    ) -> PyRefMut<'py, Self> {
        self_.enable_meshtastic = enable;
        self_
    }

    pub fn with_sportident<'py>(
        mut self_: PyRefMut<'py, Self>,
        enable: bool,
    ) -> PyRefMut<'py, Self> {
        self_.enable_sportident = enable;
        self_
    }

    pub fn with_sportident_factory<'py>(
        mut self_: PyRefMut<'py, Self>,
        factory: Option<Bound<'py, PyUsbSerialFactory>>,
    ) -> PyRefMut<'py, Self> {
        self_.sportident_factory = factory.map(|f| f.unbind());
        self_
    }

    pub fn with_tcp<'py>(mut self_: PyRefMut<'py, Self>, host: String) -> PyRefMut<'py, Self> {
        self_.meshtastic_tcp = Some(host);
        self_
    }

    pub fn with_fake_punch<'py>(
        mut self_: PyRefMut<'py, Self>,
        interval: Duration,
    ) -> PyRefMut<'py, Self> {
        self_.fake_punch_interval = Some(interval);
        self_
    }

    pub fn build(&self, py: Python<'_>) -> PyResult<MessageHandler> {
        let sportident = if let Some(ref factory_py) = self.sportident_factory {
            let factory_bound = factory_py.bind(py);
            let factory = factory_bound.borrow().clone();
            SportIdentConfig::Active(Box::new(factory))
        } else if self.enable_sportident {
            SportIdentConfig::Passive
        } else {
            SportIdentConfig::None
        };
        let usb_serial_config = UsbSerialConfig {
            enable_meshtastic: self.enable_meshtastic,
            sportident,
        };

        let mut builder = MessageHandlerBuilderRs::new()
            .with_dns(self.dns.clone())
            .with_mqtt_configs(
                self.mqtt_configs.iter().map(|config| config.clone().into()).collect(),
            )
            .with_node_infos_interval(self.node_info_interval)
            .with_meshtastic_timeout(self.meshtastic_timeout)
            .with_usb_serial_config(usb_serial_config);

        if let Some(ref host) = self.meshtastic_tcp {
            builder = builder.with_tcp(host.clone());
        }

        if let Some(interval) = self.fake_punch_interval {
            builder = builder.with_fake_punch(interval);
        }

        let message_handler_rs = builder.build();
        let inner = Arc::new(Mutex::new(message_handler_rs));
        Ok(MessageHandler { inner })
    }
}
