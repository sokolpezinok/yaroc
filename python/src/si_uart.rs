use pyo3::exceptions::{PyConnectionError, PyRuntimeError};
use pyo3::prelude::*;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use yaroc_common::punch::RawPunch;
use yaroc_receiver::serial_device_manager::SerialDeviceManager;

/// `SiUartPunchReceiver` is a Python-exposed class that receives punches from SI card readers.
#[pyclass]
pub struct SiUartPunchReceiver {
    punch_rx: Arc<Mutex<UnboundedReceiver<RawPunch>>>,
}

/// `SiUartHandler` is a Python-exposed class that manages SI card readers connected via UART.
///
/// It provides methods to add and remove devices, and to receive raw punch data from them.
#[pyclass]
pub struct SiUartHandler {
    inner: Arc<Mutex<SerialDeviceManager>>,
    punch_rx: Arc<Mutex<Option<UnboundedReceiver<RawPunch>>>>,
}

impl Default for SiUartHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[pymethods]
impl SiUartHandler {
    /// Creates a new `SiUartHandler` instance.
    ///
    /// Initializes the internal `SerialDeviceManager` and sets up a channel for receiving punches.
    #[new]
    pub fn new() -> Self {
        let (punch_tx, punch_rx) = unbounded_channel::<RawPunch>();
        let inner = SerialDeviceManager::new(None, Some(punch_tx));
        Self {
            inner: Arc::new(Mutex::new(inner)),
            punch_rx: Arc::new(Mutex::new(Some(punch_rx))),
        }
    }

    /// Retrieves the punch receiver. This can only be called once.
    pub fn punch_receiver(&self) -> PyResult<SiUartPunchReceiver> {
        let mut guard = self
            .punch_rx
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("Could not lock punch_rx".to_owned()))?;
        let rx = guard
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("Punch receiver already taken".to_owned()))?;
        Ok(SiUartPunchReceiver {
            punch_rx: Arc::new(Mutex::new(rx)),
        })
    }

    /// Asynchronously runs a background loop to automatically monitor USB hotplug events
    /// and add/remove SI devices accordingly.
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

#[pymethods]
impl SiUartPunchReceiver {
    /// Asynchronously waits for and returns the next raw punch from any connected SI device.
    ///
    /// # Arguments
    /// * `py` - Python interpreter instance.
    ///
    /// # Returns
    /// A `PyResult` containing a `RawPunch` on success, or a `PyConnectionError` if the
    /// punch channel is closed, or a `PyRuntimeError` for other issues.
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
