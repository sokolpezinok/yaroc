use std::collections::HashMap;
use std::time::Duration;

use chrono::Local;
use futures::StreamExt as _;
use futures::future::BoxFuture;
use futures::future::FutureExt as _;
use log::{error, info, warn};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

use tokio_util::sync::DropGuard;
use tokio_stream::wrappers::UnboundedReceiverStream;
use yaroc_common::punch::{RawPunch, SiPunch};
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

    /// Converts the SI UART device into a stream of `SportIdentMessage::RawPunch` items
    /// until the serial connection is closed.
    fn into_stream(self) -> impl futures::Stream<Item = Self::Output> + Send {
        futures::stream::unfold(self, |mut si_uart| async move {
            let res = si_uart.read().await;
            if let Err(Error::UartClosedError) = res {
                warn!("Removed SI UART device: {}", si_uart.rx.port());
                return None;
            }
            let punches =
                res.inspect_err(|e| error!("Failed to read punch: {e}")).unwrap_or_default();
            let msgs = futures::stream::iter(punches.into_iter().map(Into::into));
            Some((msgs, si_uart))
        })
        .flatten()
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

pub struct FakePunchConfig {
    pub interval: Duration,
    pub card: u32,
    pub code: u16,
}

impl Default for FakePunchConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            card: 46283,
            code: 47,
        }
    }
}

impl FakePunchConfig {
    /// Converts this configuration into a periodic stream of fake `SportIdentMessage::RawPunch` messages.
    pub fn into_stream(self) -> impl futures::Stream<Item = SportIdentMessage> + Send {
        info!(
            "Starting a fake SportIdent worker, sending a punch every {:?} with card {} and code {}",
            self.interval, self.card, self.code
        );
        futures::stream::unfold(self, |config| async move {
            tokio::time::sleep(config.interval).await;
            let punch = SiPunch::new_send_last_record(
                config.card,
                config.code,
                Local::now().fixed_offset(),
                18,
            );
            Some((SportIdentMessage::RawPunch(punch.raw), config))
        })
    }
}

/// The Vendor ID for Silicon Labs, commonly used by SportIdent USB adapters.
pub const SILABS_VID: u16 = 0x10c4;

/// A factory for creating and managing background tasks for SportIdent serial devices.
pub struct SportIdentFactory {
    devices: HashMap<String, (DropGuard, String)>,
    si_tx: UnboundedSender<SportIdentMessage>,
}

impl SportIdentFactory {
    /// Creates a new `SportIdentFactory` and a stream of incoming `SportIdentMessage`s.
    pub fn new() -> (Self, impl futures::Stream<Item = SportIdentMessage> + Send) {
        let (si_tx, si_rx) = unbounded_channel();
        (
            Self {
                devices: HashMap::new(),
                si_tx,
            },
            UnboundedReceiverStream::new(si_rx),
        )
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
        let serialport::SerialPortType::UsbPort(usb_info) = &port.port_type else {
            return false;
        };
        dev_serial == usb_info.serial_number.as_deref() && usb_info.vid == SILABS_VID
    }

    /// Spawns the serial reading background loop for the given SportIdent serial device and
    /// registers its drop guard.
    pub fn add_sportident_device_inner<S>(&mut self, si_serial: S, device_node: &str, port: &str)
    where
        S: UsbSerialTrait<Output = SportIdentMessage> + std::fmt::Display + Send + 'static,
    {
        let drop_guard = si_serial.spawn_serial(self.si_tx.clone());

        self.devices.insert(device_node.to_owned(), (drop_guard, port.to_owned()));
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
    /// connection background task, and registers its drop guard.
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

    /// Removes a SportIdent serial device, cancelling its background task via its drop guard and
    /// sending a device-removed event.
    fn remove_device(&mut self, device_node: &str) -> bool {
        let Some((_guard, port)) = self.devices.remove(device_node) else {
            return false;
        };
        let _ = self.si_tx.send(SportIdentMessage::DeviceEvent {
            added: false,
            device: port,
        });
        true
    }

    /// Checks if a connection background task is currently running for the given device node.
    fn is_running(&self, device_node: &str) -> bool {
        self.devices.contains_key(device_node)
    }

    /// Name
    fn name(&self) -> &'static str {
        "SportIdent"
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
        let (mut factory, _si_stream) = SportIdentFactory::new();

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
