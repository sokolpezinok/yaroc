use std::{future, sync::Arc};

use log::{error, info};
use pyo3::exceptions::{PyConnectionError, PyRuntimeError};
use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::future_into_py;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader, ReadHalf, WriteHalf, split};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::{io::AsyncWriteExt, sync::Mutex};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

use crate::punch::SiPunchLog;

/// A client for interacting with a serial port, typically for SportIdent (SI) devices.
///
/// This struct manages the reading from and writing to a serial port,
/// allowing communication with SI devices and responding as a blue SRR (SportIdent Reader).
#[pyclass]
pub struct SerialClient {
    computer_rx: Arc<Mutex<BufReader<ReadHalf<SerialStream>>>>,
    computer_tx: Arc<Mutex<WriteHalf<SerialStream>>>,
    mini_reader: Arc<Mutex<Option<SerialStream>>>,
    mini_reader_connect_rx: Arc<Mutex<UnboundedReceiver<String>>>,
    mini_reader_connect_tx: UnboundedSender<String>,
}

const FIRST_RESPONSE: &[u8] = b"\xff\x02\xf0\x03\x12\x8cMb?\x03";
const FINAL_RESPONSE: &[u8] = b"\xff\x02\x83\x83\x12\x8c\x00\r\x00\x12\x8c\x04450\x16\x0b\x0fo!\xff\xff\xff\x02\x06\x00\x1b\x17?\x18\x18\x06)\x08\x05>\xfe\n\xeb\n\xeb\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\x92\xba\x1aB\x01\xff\xff\xe1\xff\xff\xff\xff\xff\x01\x01\x01\x0b\x07\x0c\x00\r]\x0eD\x0f\xec\x10-\x11;\x12s\x13#\x14;\x15\x01\x19\x1d\x1a\x1c\x1b\xc7\x1c\x00\x1d\xb0!\xb6\"\x10#\xea$\n%\x00&\x11,\x88-1.\x0b\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xf9\xc3\x03";
const ETX: u8 = 0x03;

impl SerialClient {
    /// Responds to orienteering software as a blue SportIdent SRR dongle.
    ///
    /// This function handles the communication protocol with orienteering software,
    /// specifically MeOS and SportIdent Reader, by sending predefined responses.
    async fn respond_as_blue_srr(
        computer_rx: Arc<Mutex<BufReader<ReadHalf<SerialStream>>>>,
        computer_tx: Arc<Mutex<WriteHalf<SerialStream>>>,
        mini_reader_connect_rx: Arc<Mutex<UnboundedReceiver<String>>>,
    ) -> Option<String> {
        let mut rx = computer_rx.lock().await;
        let mut query = Vec::new();
        let mut mini_reader_connect_rx = mini_reader_connect_rx.lock().await;

        tokio::select! {
            _len = rx.read_until(ETX, &mut query) => {
                match query.as_slice() {
                    b"\xff\x02\x02\xf0\x01Mm\n\x03" => {
                        info!("Responding to orienteering software - MeOS");
                    }
                    b"\xff\x02\xf0\x01Mm\n\x03" => {
                        info!("Responding to orienteering software - SportIdent Reader");
                    }
                    _ => {
                        error!("Contacted by unknown orienteering software");
                        return None;
                    }
                }

                let mut tx = computer_tx.lock().await;
                let _ = tx
                    .write_all(FIRST_RESPONSE)
                    .await
                    .inspect_err(|e| error!("Communication with software failed: {e}"));
                let mut data = Vec::new();
                let _len = rx.read_until(ETX, &mut data).await.unwrap();

                match data.as_slice() {
                    b"\x02\x83\x02\x00\x80\xbf\x17\x03" | b"\xff\x02\x83\x02\x00\x80\xbf\x17\x03" => {}
                    _ => {
                        error!("Communicating with software failed");
                        return None;
                    }
                }
                let _ = tx
                    .write_all(FINAL_RESPONSE)
                    .await
                    .inspect_err(|e| error!("Communication with software failed: {e}"));
                None
            }
            device = mini_reader_connect_rx.recv() => {
                device
            }
        }
    }

    /// Handles communication when a mini-reader is connected.
    ///
    /// This function continuously reads from both the computer's serial port and the mini-reader,
    /// forwarding data between them. It acts as a bridge for communication.
    ///
    /// # Arguments
    /// * `computer_rx` - An `Arc<Mutex<BufReader<ReadHalf<SerialStream>>>>` for reading from the computer.
    /// * `computer_tx` - An `Arc<Mutex<WriteHalf<SerialStream>>>` for writing to the computer.
    /// * `mini_reader` - A mutable reference to the `SerialStream` connected to the mini-reader.
    async fn respond_as_mini_reader(
        computer_rx: Arc<Mutex<BufReader<ReadHalf<SerialStream>>>>,
        computer_tx: Arc<Mutex<WriteHalf<SerialStream>>>,
        mini_reader: &mut SerialStream,
        mini_reader_connect_rx: Arc<Mutex<UnboundedReceiver<String>>>,
    ) -> Option<String> {
        loop {
            let mut reader_buffer = Vec::with_capacity(280);
            let mut buffer = Vec::with_capacity(280);
            let mut rx = computer_rx.lock().await;
            let mut mini_reader_connect_rx = mini_reader_connect_rx.lock().await;
            tokio::select! {
                len = mini_reader.read_buf(&mut reader_buffer) => {
                    match len {
                        Ok(0) => {
                            // Disconnected
                            return None;
                        }
                        Err(e) => {
                            error!("Error while reading from mini reader: {e}");
                        }
                        _ => {
                            let mut tx = computer_tx.lock().await;
                            let _ = tx.write_all(reader_buffer.as_slice()).await.inspect_err(|_| error!("Error writing into serial port"));
                        }
                    };
                    // TODO: continue the full transaction and only then release computer_tx.
                }
                len = rx.read_buf(&mut buffer) => {
                    let _ = len.inspect_err(|e| error!("Error while reading the computer: {e}"));
                    let _ = mini_reader.write_all(buffer.as_slice()).await.inspect_err(|e| error!("Error writing back to mini-Reader: {e}"));
                }
                device = mini_reader_connect_rx.recv() => {
                    return device;
                }
            }
        }
    }

    fn connect_to_mini_reader(port: String) -> PyResult<SerialStream> {
        let builder = tokio_serial::new(&port, 38400);
        info!("Connected to mini-reader at {port}");
        builder
            .open_native_async()
            .map_err(|e| PyConnectionError::new_err(format!("Error connecting to {}: {e}", port)))
    }
}

#[pymethods]
impl SerialClient {
    /// Creates a new `SerialClient` instance and connects to the specified serial port.
    ///
    /// # Arguments
    /// * `computer_port` - The path to the serial port (e.g., "/dev/ttyUSB0").
    /// * `py` - Python interpreter instance.
    ///
    /// # Returns
    /// A `PyResult` containing a `Bound<'a, PyAny>` which resolves to a `SerialClient` instance.
    #[staticmethod]
    pub fn create<'a>(computer_port: String, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        future_into_py::<_, SerialClient>(py, async move {
            let builder = tokio_serial::new(&computer_port, 38400);
            let computer_serial = builder.open_native_async().map_err(|e| {
                PyConnectionError::new_err(format!("Error connecting to {}: {e}", computer_port))
            })?;
            info!("Connected to SRR sink at {computer_port}");
            let (rx, tx) = split(computer_serial);
            let rx = BufReader::new(rx);
            let (mini_reader_connect_tx, mini_reader_connect_rx) = unbounded_channel();

            Ok(Self {
                computer_rx: Arc::new(Mutex::new(rx)),
                computer_tx: Arc::new(Mutex::new(tx)),
                mini_reader: Arc::default(),
                mini_reader_connect_rx: Arc::new(Mutex::new(mini_reader_connect_rx)),
                mini_reader_connect_tx,
            })
        })
    }

    /// Starts an asynchronous loop to continuously respond as a blue SRR dongle.
    ///
    /// This method runs an infinite loop that calls `respond_as_blue_srr` to handle
    /// incoming communication from orienteering software.
    ///
    /// # Arguments
    /// * `py` - Python interpreter instance.
    ///
    /// # Returns
    /// A `PyResult` containing a `Bound<'a, PyAny>` which resolves when the loop starts.
    pub fn r#loop<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let computer_rx = self.computer_rx.clone();
        let computer_tx = self.computer_tx.clone();
        let mini_reader = self.mini_reader.clone();
        let mini_reader_connect_rx = self.mini_reader_connect_rx.clone();
        future_into_py::<_, ()>(py, async move {
            loop {
                let mut mini_reader = mini_reader.lock().await;
                let device = if let Some(mini_reader_stream) = mini_reader.as_mut() {
                    Self::respond_as_mini_reader(
                        computer_rx.clone(),
                        computer_tx.clone(),
                        mini_reader_stream,
                        mini_reader_connect_rx.clone(),
                    )
                    .await
                } else {
                    Self::respond_as_blue_srr(
                        computer_rx.clone(),
                        computer_tx.clone(),
                        mini_reader_connect_rx.clone(),
                    )
                    .await
                };
                *mini_reader = device.map(Self::connect_to_mini_reader).transpose()?;
            }
        })
    }

    /// Adds a mini-reader to the client.
    ///
    /// This method sends the port of the mini-reader to the main loop, which will then
    /// connect to it.
    ///
    /// # Arguments
    /// * `py` - Python interpreter instance.
    /// * `port` - The path to the serial port of the mini-reader (e.g., "/dev/ttyUSB1").
    ///
    /// # Returns
    /// A `PyResult` containing a `Bound<'a, PyAny>` which resolves when the port is sent.
    pub fn add_mini_reader<'a>(&self, py: Python<'a>, port: String) -> PyResult<Bound<'a, PyAny>> {
        let tx = self.mini_reader_connect_tx.clone();
        future_into_py(py, async move {
            tx.send(port).map_err(|e| {
                PyRuntimeError::new_err(format!("Error sending mini reader port: {e}"))
            })?;
            Ok(())
        })
    }

    /// Sends a SportIdent punch log via the serial port.
    ///
    /// # Arguments
    /// * `py` - Python interpreter instance.
    /// * `punch_log` - The `SiPunchLog` containing the raw punch data to send.
    ///
    /// # Returns
    /// A `PyResult` containing a `Bound<'a, PyAny>` which resolves when the punch is sent.
    pub fn send_punch<'a>(
        &self,
        py: Python<'a>,
        punch_log: &SiPunchLog,
    ) -> PyResult<Bound<'a, PyAny>> {
        let computer_tx = self.computer_tx.clone();
        let raw_punch = punch_log.punch.raw;

        future_into_py(py, async move {
            let mut tx = computer_tx.lock().await;
            tx.write_all(&raw_punch).await.map_err(|e| {
                PyRuntimeError::new_err(format!("Error sending punch via serial port: {e}"))
            })?;
            info!("Punch sent via serial port");
            Ok(())
        })
    }

    /// Placeholder for sending status information.
    ///
    /// This method is currently a placeholder and does not perform any action.
    ///
    /// # Arguments
    /// * `py` - Python interpreter instance.
    /// * `_status` - Placeholder for status object.
    /// * `_mac_addr` - Placeholder for MAC address string.
    ///
    /// # Returns
    /// A `PyResult` containing a `Bound<'a, PyAny>` which resolves immediately to `true`.
    pub fn send_status<'a>(
        &self,
        py: Python<'a>,
        _status: &Bound<'_, PyAny>,
        _mac_addr: &str,
    ) -> PyResult<Bound<'a, PyAny>> {
        future_into_py(py, future::ready(Ok(true)))
    }
}
