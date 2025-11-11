use log::{error, info};
use pyo3::exceptions::{PyConnectionError, PyRuntimeError};
use pyo3::prelude::*;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use yaroc_common::punch::RawPunch;
use yaroc_common::si_uart::SiUart;
use yaroc_receiver::serial_device_manager::SerialDeviceManager;
use yaroc_receiver::si_uart::TokioSerial;

type SiUartTokio = SiUart<TokioSerial>;

#[pyclass]
pub struct SiUartHandler {
    inner: Arc<Mutex<SerialDeviceManager<SiUartTokio>>>,
    punch_rx: Arc<Mutex<UnboundedReceiver<RawPunch>>>,
}

impl Default for SiUartHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[pymethods]
impl SiUartHandler {
    #[new]
    pub fn new() -> Self {
        let (punch_tx, punch_rx) = unbounded_channel::<RawPunch>();
        let inner = SerialDeviceManager::new(punch_tx);
        Self {
            inner: Arc::new(Mutex::new(inner)),
            punch_rx: Arc::new(Mutex::new(punch_rx)),
        }
    }

    pub fn add_device<'a>(
        &mut self,
        py: Python<'a>,
        port: String,
        device_node: String,
    ) -> PyResult<Bound<'a, PyAny>> {
        let mutex = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py::<_, ()>(py, async move {
            match TokioSerial::new(port.as_str()) {
                Ok(serial) => {
                    let si_uart = SiUart::new(serial);
                    mutex.lock().await.add_device(si_uart, &device_node);
                    info!("Connected to SI UART device at {port}",);
                }
                Err(err) => {
                    error!("Error connecting to {port}: {err}");
                }
            };
            Ok(())
        })
    }

    pub fn remove_device(&mut self, device_node: String) -> PyResult<bool> {
        self.inner
            .try_lock()
            .map_err(|_| {
                PyRuntimeError::new_err("Failed to lock meshtastic device handler".to_owned())
            })
            .map(|mut handler| handler.remove_device(device_node))
    }

    pub fn next_punch<'a>(&'a self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let rx = self.punch_rx.clone();
        pyo3_async_runtimes::tokio::future_into_py::<_, RawPunch>(py, async move {
            rx.lock()
                .await
                .recv()
                .await
                .ok_or(PyConnectionError::new_err("Channel closed".to_owned()))
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))
        })
    }
}
