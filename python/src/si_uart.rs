use log::{error, info};
use pyo3::exceptions::{PyConnectionError, PyRuntimeError};
use pyo3::prelude::*;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc::UnboundedReceiver;
use yaroc_common::punch::RawPunch;

use yaroc_receiver::si_uart::{SiUartHandler as SiUartHandlerRs, TokioSerial};

#[pyclass]
pub struct SiUartHandler {
    inner: SiUartHandlerRs,
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
        let (inner, punch_rx) = SiUartHandlerRs::new();
        Self {
            inner,
            punch_rx: Arc::new(Mutex::new(punch_rx)),
        }
    }

    pub fn add_device(&mut self, port: String, device_node: String) {
        match TokioSerial::new(port.as_str()) {
            Ok(serial) => {
                self.inner.add_device(serial, &device_node);
                info!("Connected to SI UART device at {port}",);
            }
            Err(err) => {
                error!("Error connecting to {port}: {err}");
            }
        }
    }

    pub fn remove_device(&mut self, device_node: String) -> bool {
        self.inner.remove_device(device_node)
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
