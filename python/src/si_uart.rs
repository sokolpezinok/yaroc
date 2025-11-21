use log::info;
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

/// `SiUartHandler` is a Python-exposed class that manages SI card readers connected via UART.
///
/// It provides methods to add and remove devices, and to receive raw punch data from them.
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
    /// Creates a new `SiUartHandler` instance.
    ///
    /// Initializes the internal `SerialDeviceManager` and sets up a channel for receiving punches.
    #[new]
    pub fn new() -> Self {
        let (punch_tx, punch_rx) = unbounded_channel::<RawPunch>();
        let inner = SerialDeviceManager::new(punch_tx);
        Self {
            inner: Arc::new(Mutex::new(inner)),
            punch_rx: Arc::new(Mutex::new(punch_rx)),
        }
    }

    /// Adds a new SI UART device to be managed.
    ///
    /// This method attempts to connect to the specified serial port and, upon successful connection,
    /// registers the device with the `SerialDeviceManager`.
    ///
    /// # Arguments
    /// * `py` - Python interpreter instance.
    /// * `port` - The serial port name (e.g., "/dev/ttyUSB0" or "COM1").
    /// * `device_node` - A unique identifier for the device.
    ///
    /// # Returns
    /// A `PyResult` indicating success or an error if the connection fails.
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
                    Ok(())
                }
                Err(err) => Err(PyConnectionError::new_err(format!(
                    "Error connecting to {port}: {err}"
                ))),
            }
        })
    }

    /// Removes a previously added SI UART device.
    ///
    /// # Arguments
    /// * `device_node` - The unique identifier of the device to remove.
    ///
    /// # Returns
    /// `Ok(true)` if the device was successfully removed, `Ok(false)` if not found,
    /// or a `PyRuntimeError` if the handler is locked.
    pub fn remove_device(&mut self, device_node: String) -> PyResult<bool> {
        self.inner
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("Failed to lock SI UART handler".to_owned()))
            .map(|mut handler| handler.remove_device(device_node))
    }

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
