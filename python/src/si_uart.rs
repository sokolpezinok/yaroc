use futures_util::StreamExt;
use log::{debug, error, info};
use nusb::hotplug::HotplugEvent;
use pyo3::exceptions::{PyConnectionError, PyRuntimeError};
use pyo3::prelude::*;
use serialport::SerialPortType;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use yaroc_common::punch::RawPunch;
use yaroc_common::si_uart::SiUart;
use yaroc_receiver::serial_device_manager::SerialDeviceManager;
use yaroc_receiver::si_uart::TokioSerial;

const SI_LABS: u16 = 0x10c4;
type SiUartTokio = SiUart<TokioSerial>;

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
    inner: Arc<Mutex<SerialDeviceManager<SiUartTokio>>>,
    punch_rx: Arc<Mutex<Option<UnboundedReceiver<RawPunch>>>>,
}

impl Default for SiUartHandler {
    fn default() -> Self {
        Self::new()
    }
}

// Pure Rust (non-Python) methods
impl SiUartHandler {
    /// Adds a new SI UART device to be managed.
    ///
    /// This method attempts to connect to the specified serial port and, upon successful connection,
    /// registers the device with the `SerialDeviceManager`.
    ///
    /// # Arguments
    /// * `manager` - A SerialDeviceManager instance.
    /// * `port` - The serial port name (e.g., "/dev/ttyUSB0" or "COM1").
    /// * `device_node` - A unique identifier for the device.
    ///
    /// # Returns
    /// A `Result` indicating success or an error if the connection fails.
    fn add_device_inner(
        manager: &mut SerialDeviceManager<SiUartTokio>,
        port: &str,
        device_node: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let serial = TokioSerial::new(port)?;
        let si_uart = SiUart::new(serial);
        manager.add_device(si_uart, device_node);
        info!("Connected to SI UART device at {port}");
        Ok(())
    }

    /// Removes a previously added SI UART device.
    ///
    /// # Arguments
    /// * `manager` - A SerialDeviceManager instance.
    /// * `device_node` - The unique identifier of the device to remove.
    ///
    /// # Returns
    /// A boolean indicating whether the device was successfully removed.
    fn remove_device_inner(
        manager: &mut SerialDeviceManager<SiUartTokio>,
        device_node: &str,
    ) -> bool {
        manager.remove_device(device_node.to_owned())
    }

    /// Detects the serial port name and device ID for a given USB device.
    ///
    /// This method checks if the USB device is manufactured by Silicon Labs (matching the vendor ID
    /// `SI_LABS`). If so, it scans the available system serial ports and matches them by comparing
    /// serial numbers and vendor IDs.
    ///
    /// # Arguments
    /// * `dev` - A reference to the `nusb::DeviceInfo` of the USB device to inspect.
    ///
    /// # Returns
    /// An `Option` containing a tuple of `(port_name, device_id)` if a matching serial port
    /// is found, or `None` otherwise.
    fn detect_port(dev: &nusb::DeviceInfo) -> Option<(String, String)> {
        if dev.vendor_id() == SI_LABS {
            let Ok(ports) = serialport::available_ports() else {
                return None;
            };
            for port in ports {
                debug!("{port:?}");
                if let SerialPortType::UsbPort(usb_info) = port.port_type
                    && usb_info.vid == SI_LABS
                {
                    let sn_matches = match (dev.serial_number(), &usb_info.serial_number) {
                        (Some(dev_serial_n), Some(usb_serial_n)) => dev_serial_n == usb_serial_n,
                        (None, None) => true,
                        _ => false,
                    };
                    if sn_matches {
                        return Some((port.port_name, format!("{:?}", dev.id())));
                    }
                }
            }
        }
        None
    }

    /// Monitors USB hotplug events and manages SI UART devices dynamically.
    ///
    /// This method performs an initial scan of currently connected USB devices to find and register
    /// any matching SI UART devices. It then listens indefinitely for USB connection and disconnection
    /// events, registering new devices as they are plugged in (after a brief delay to allow the OS
    /// to initialize the serial port) and removing devices when they are unplugged.
    ///
    /// # Arguments
    /// * `manager` - A mutable reference to the `SerialDeviceManager` which holds the active devices.
    ///
    /// # Returns
    /// A `Result` that is `Ok(())` on successful monitoring loop termination, or an error if
    /// the hotplug watcher fails to initialize.
    async fn monitor_usb_devices(
        manager: &mut SerialDeviceManager<SiUartTokio>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting USB SportIdent device manager");
        if let Ok(devices) = nusb::list_devices().await {
            for dev in devices {
                if let Some((port_name, dev_id)) = Self::detect_port(&dev) {
                    let _ = SiUartHandler::add_device_inner(manager, &port_name, &dev_id)
                        .inspect_err(|err| {
                            error!("Failed to connect to SI UART device at {port_name}: {err}")
                        });
                }
            }
        }

        let mut watcher = nusb::watch_devices()?;
        while let Some(event) = watcher.next().await {
            match event {
                HotplugEvent::Connected(dev) => {
                    // Give the OS TTY subsystem a brief moment to register the node
                    // TODO: is there a way to avaoid this sleep? Listen to tty subsystem events?
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    if let Some((port_name, dev_id)) = Self::detect_port(&dev) {
                        let _ = SiUartHandler::add_device_inner(manager, &port_name, &dev_id)
                            .inspect_err(|err| {
                                error!("Failed to connect to SI UART device at {port_name}: {err}")
                            });
                    }
                }
                HotplugEvent::Disconnected(dev_id) => {
                    SiUartHandler::remove_device_inner(manager, &format!("{:?}", dev_id));
                    info!("Disconnected SI UART device {dev_id:?}");
                }
            }
        }

        Ok(())
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
            Self::monitor_usb_devices(&mut manager)
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
