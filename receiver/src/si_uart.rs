use std::collections::HashMap;

use futures::future::BoxFuture;
use futures::future::FutureExt as _;
use log::{error, info, warn};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_serial::{SerialPortBuilderExt, SerialStream};

use tokio_util::sync::CancellationToken;
use yaroc_common::punch::RawPunch;
use yaroc_common::si_uart::{BAUD_RATE, SiUart};
use yaroc_common::{error::Error, si_uart::RxWithIdle};

use crate::usb_serial_manager::{UsbSerialFactory, UsbSerialTrait};

pub struct TokioSerial {
    serial: SerialStream,
    port: String,
}

impl TokioSerial {
    /// Creates a new `TokioSerial` instance from a serial port path.
    ///
    /// # Arguments
    ///
    /// * `port` - The path to the serial port, e.g., `/dev/ttyUSB0`.
    ///
    /// # Returns
    ///
    /// A `Result` containing the new `TokioSerial` instance or an error if the port cannot be
    /// opened.
    pub fn new(port: &str) -> crate::Result<Self> {
        let builder = tokio_serial::new(port, BAUD_RATE);
        let serial =
            builder.open_native_async().map_err(|_| crate::error::Error::ConnectionError)?;
        Ok(Self {
            serial,
            port: port.to_owned(),
        })
    }
}

impl RxWithIdle for TokioSerial {
    /// Reads from the serial port until a timeout is hit, which is considered to be an idle state.
    ///
    /// This is a thin wrapper around `tokio_serial::SerialStream::read`.
    async fn read_until_idle(&mut self, buf: &mut [u8]) -> yaroc_common::Result<usize> {
        self.serial.read(buf).await.map_err(|_| Error::UartReadError)
    }

    /// Returns the port path of the serial connection.
    fn port(&self) -> &str {
        &self.port
    }
}

impl UsbSerialTrait for SiUart<TokioSerial> {
    type Output = SportIdentMessage;

    /// Read loop that consumes punches from the SI UART device and sends them as
    /// `SportIdentMessage::RawPunch` through the channel until the serial connection is closed.
    async fn inner_loop(mut self, tx: UnboundedSender<Self::Output>) {
        loop {
            let punch = self.read().await;
            match punch {
                Ok(punches) => {
                    for punch in punches {
                        tx.send(punch.into()).expect("Channel unexpectedly closed");
                    }
                }
                Err(err) => match err {
                    Error::UartClosedError => {
                        error!("Removed SI UART device: {}", self);
                        break;
                    }
                    e => {
                        error!("Failed to read punch: {e}");
                    }
                },
            }
        }
    }
}

/// An event or punch message emitted by a SportIdent device.
#[derive(Debug, Clone)]
pub enum SportIdentMessage {
    /// A raw punch.
    RawPunch(RawPunch),
    /// A hardware connection event indicating whether a device was added or removed.
    DeviceEvent { added: bool, device: String },
}

impl From<RawPunch> for SportIdentMessage {
    fn from(punch: RawPunch) -> Self {
        Self::RawPunch(punch)
    }
}

/// The Vendor ID for Silicon Labs, commonly used by SportIdent USB adapters.
pub const SILABS_VID: u16 = 0x10c4;

/// A factory for creating and managing background tasks for SportIdent serial devices.
pub struct SportIdentFactory {
    devices: HashMap<String, (CancellationToken, String)>,
    si_tx: UnboundedSender<SportIdentMessage>,
}

impl SportIdentFactory {
    /// Creates a new `SportIdentFactory` that forwards SportIdent messages through the given sender.
    pub fn new(si_tx: UnboundedSender<SportIdentMessage>) -> Self {
        Self {
            devices: HashMap::new(),
            si_tx,
        }
    }

    /// Checks if a USB device matches a SportIdent device by checking serial numbers and ensuring
    /// the vendor ID matches Silicon Labs.
    pub fn detect_device(dev: &nusb::DeviceInfo, port: &serialport::SerialPortInfo) -> bool {
        Self::detect_device_inner(dev.serial_number(), port)
    }

    pub fn detect_device_inner(
        dev_serial: Option<&str>,
        port: &serialport::SerialPortInfo,
    ) -> bool {
        if let serialport::SerialPortType::UsbPort(usb_info) = &port.port_type {
            let sn_matches = match (dev_serial, &usb_info.serial_number) {
                (Some(dev_serial_n), Some(usb_serial_n)) => dev_serial_n == usb_serial_n,
                (None, None) => true,
                _ => false,
            };
            sn_matches && usb_info.vid == SILABS_VID
        } else {
            false
        }
    }

    /// Spawns the serial reading background loop for the given SportIdent serial device and
    /// registers its cancellation token.
    pub fn add_sportident_device_inner<S>(&mut self, si_serial: S, device_node: &str, port: &str)
    where
        S: UsbSerialTrait<Output = SportIdentMessage> + std::fmt::Display + Send + 'static,
    {
        let token = si_serial.spawn_serial(self.si_tx.clone());

        self.devices.insert(device_node.to_owned(), (token, port.to_owned()));
        let _ = self.si_tx.send(SportIdentMessage::DeviceEvent {
            added: true,
            device: port.to_owned(),
        });
    }
}

impl UsbSerialFactory for SportIdentFactory {
    /// Detects if a USB device matches a SportIdent serial device.
    fn detect_device(&self, dev: &nusb::DeviceInfo, port: &serialport::SerialPortInfo) -> bool {
        Self::detect_device(dev, port)
    }

    /// Asynchronously connects to a SportIdent serial device at the given port, spawns its
    /// connection background task, and registers its cancellation token.
    fn add_device<'a>(
        &'a mut self,
        port: &'a str,
        device_node: &'a str,
    ) -> BoxFuture<'a, Result<(), Box<dyn std::error::Error + Send + Sync>>> {
        async move {
            let serial = TokioSerial::new(port)?;
            let si_uart = SiUart::new(serial);
            self.add_sportident_device_inner(si_uart, device_node, port);
            info!("Connected to SI UART device at {port}");
            Ok(())
        }
        .boxed()
    }

    /// Removes a SportIdent serial device by triggering its background task cancellation and
    /// sending a device-removed event.
    fn remove_device(&mut self, device_node: &str) -> bool {
        if let Some((token, port)) = self.devices.remove(device_node) {
            token.cancel();
            warn!("Removed SI UART device at {port}");
            let _ = self.si_tx.send(SportIdentMessage::DeviceEvent {
                added: false,
                device: port,
            });
            true
        } else {
            false
        }
    }

    /// Checks if a connection background task is currently running for the given device node.
    fn is_running(&self, device_node: &str) -> bool {
        self.devices.contains_key(device_node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serialport::{SerialPortInfo, SerialPortType, UsbPortInfo};

    #[test]
    fn test_detect_device_inner() {
        let mut usb_info = UsbPortInfo {
            vid: SILABS_VID,
            pid: 0x5678,
            serial_number: Some("12345".to_owned()),
            manufacturer: None,
            product: None,
        };

        let mut port = SerialPortInfo {
            port_name: "/dev/ttyUSB0".to_owned(),
            port_type: SerialPortType::UsbPort(usb_info.clone()),
        };

        // Match
        assert!(SportIdentFactory::detect_device_inner(Some("12345"), &port));

        // Mismatch serial
        assert!(!SportIdentFactory::detect_device_inner(
            Some("54321"),
            &port
        ));

        // Mismatch vid
        usb_info.vid = 0x1234;
        port.port_type = SerialPortType::UsbPort(usb_info.clone());
        assert!(!SportIdentFactory::detect_device_inner(
            Some("12345"),
            &port
        ));

        // Not USB
        port.port_type = SerialPortType::PciPort;
        assert!(!SportIdentFactory::detect_device_inner(
            Some("12345"),
            &port
        ));
    }

    #[tokio::test]
    async fn test_sportident_factory_management() {
        let (si_tx, mut _si_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut factory = SportIdentFactory::new(si_tx);

        let (_rx_tx, rx_rx) = tokio::sync::mpsc::channel(1);
        let fake_serial =
            crate::test_utils::FakeSportIdentSerial::new("/dev/ttyUSB0".to_owned(), rx_rx);

        assert!(!factory.is_running("/dev/ttyUSB0"));

        factory.add_sportident_device_inner(fake_serial, "/dev/ttyUSB0", "/dev/ttyUSB0");
        assert!(factory.is_running("/dev/ttyUSB0"));

        let removed = factory.remove_device("/dev/ttyUSB0");
        assert!(removed);
        assert!(!factory.is_running("/dev/ttyUSB0"));

        let removed_again = factory.remove_device("/dev/ttyUSB0");
        assert!(!removed_again);
    }
}
