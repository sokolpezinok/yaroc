use std::{future, sync::Arc};

use log::info;
use pyo3::exceptions::{PyConnectionError, PyRuntimeError};
use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::future_into_py;
use tokio::{io::AsyncWriteExt, sync::Mutex};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

use crate::punch::SiPunchLog;

#[pyclass]
pub struct SerialClient {
    computer_serial: Arc<Mutex<SerialStream>>,
}

#[pymethods]
impl SerialClient {
    #[new]
    pub fn new(computer_port: &str) -> PyResult<Self> {
        let builder = tokio_serial::new(computer_port, 38400);
        let computer_serial = builder
            .open_native_async()
            .map_err(|e| PyConnectionError::new_err(e.to_string()))?;
        Ok(Self {
            computer_serial: Arc::new(Mutex::new(computer_serial)),
        })
    }

    pub fn send_punch<'a>(
        &mut self,
        py: Python<'a>,
        punch_log: &SiPunchLog,
    ) -> PyResult<Bound<'a, PyAny>> {
        let computer_serial = self.computer_serial.clone();
        let raw_punch = punch_log.punch.raw;

        future_into_py::<_, ()>(py, async move {
            let mut serial = computer_serial.lock().await;
            serial
                .write_all(&raw_punch)
                .await
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
            info!("Punch sent via serial port");
            Ok(())
        })
    }

    pub fn send_status<'a>(
        &'a self,
        py: Python<'a>,
        _status: &Bound<'_, PyAny>,
        _mac_addr: &str,
    ) -> PyResult<Bound<'a, PyAny>> {
        future_into_py(py, future::ready(Ok(true)))
    }
}
