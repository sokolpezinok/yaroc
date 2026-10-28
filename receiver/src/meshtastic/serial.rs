use std::collections::HashMap;
use std::fmt::Display;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::{FutureExt as _, Stream};
use log::info;
use meshtastic::protobufs::ServiceEnvelope;
use meshtastic::utils::stream;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_util::sync::DropGuard;

use super::connection::MeshtasticConnection;
use crate::si_uart::SILABS_VID;
use crate::system_info::MacAddress;
use crate::usb_serial_manager::{UsbSerialFactory, UsbSerialTrait};

/// A connection to a Meshtastic device.
pub struct MeshtasticSerial {
    connection: MeshtasticConnection,
}

impl MeshtasticSerial {
    /// Creates a new Meshtastic serial connection.
    pub async fn new(
        port: &str,
        timeout: Duration,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let serial_stream = stream::build_serial_stream(port.to_owned(), None, None, None)?;
        let connection =
            MeshtasticConnection::connect_stream(serial_stream, timeout, port.to_owned()).await?;
        info!(
            "Connected to Meshtastic device: {} at {port}",
            connection.mac_address
        );

        Ok(Self { connection })
    }

    /// Returns the MAC address of the device.
    pub fn mac_address(&self) -> MacAddress {
        self.connection.mac_address
    }
}

impl UsbSerialTrait for MeshtasticSerial {
    type Output = ServiceEnvelope;

    /// Converts the Meshtastic device into a stream of `ServiceEnvelope` messages.
    fn into_stream(self) -> impl futures::Stream<Item = ServiceEnvelope> + Send {
        self.connection.into_stream()
    }
}

impl Display for MeshtasticSerial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Meshtastic serial device {}", self.mac_address())
    }
}

/// A factory for creating and managing background tasks for Meshtastic serial devices.
pub struct MeshtasticFactory {
    devices: HashMap<String, DropGuard>,
    mesh_tx: UnboundedSender<ServiceEnvelope>,
}

impl MeshtasticFactory {
    /// Creates a new `MeshtasticFactory` and a stream of incoming `ServiceEnvelope` messages.
    pub fn new() -> (Self, impl Stream<Item = ServiceEnvelope> + Send) {
        let (mesh_tx, mesh_rx) = unbounded_channel();
        let stream = futures::stream::unfold(mesh_rx, |mut rx| async move {
            rx.recv().await.map(|msg| (msg, rx))
        });
        (
            Self {
                devices: HashMap::new(),
                mesh_tx,
            },
            stream,
        )
    }

    /// Spawns the serial reading background loop for the given Meshtastic serial device and
    /// registers its drop guard.
    pub fn add_meshtastic_device_inner<M>(&mut self, msh_serial: M, device_node: &str)
    where
        M: UsbSerialTrait<Output = ServiceEnvelope> + Send + Display + 'static,
    {
        let drop_guard = msh_serial.spawn_serial(self.mesh_tx.clone());
        self.devices.insert(device_node.to_owned(), drop_guard);
    }

    /// Inner logic for device detection that can be unit tested without a `nusb::DeviceInfo`.
    fn detect_device_inner(dev_serial: Option<&str>, port: &serialport::SerialPortInfo) -> bool {
        let serialport::SerialPortType::UsbPort(usb_info) = &port.port_type else {
            return false;
        };
        let sn_matches = dev_serial == usb_info.serial_number.as_deref();
        let is_sportident = usb_info.vid == SILABS_VID;
        sn_matches
            && !is_sportident
            && (port.port_name.contains("ACM") || port.port_name.contains("COM"))
    }
}

impl UsbSerialFactory for MeshtasticFactory {
    /// Checks if a USB device matches a Meshtastic serial device by comparing serial numbers
    /// and ensuring the port name is an ACM/COM interface.
    fn detect_device(&self, dev: &nusb::DeviceInfo, port: &serialport::SerialPortInfo) -> bool {
        Self::detect_device_inner(dev.serial_number(), port)
    }

    /// Asynchronously connects to a Meshtastic serial device at the given port, creates
    /// a new `MeshtasticSerial` instance, and adds it to the active device map.
    fn add_device<'a>(
        &'a mut self,
        port: &'a str,
        device_node: &'a str,
    ) -> BoxFuture<'a, Result<(), Box<dyn std::error::Error + Send + Sync>>> {
        async move {
            let msh_serial =
                MeshtasticSerial::new(port, std::time::Duration::from_secs(12)).await?;
            self.add_meshtastic_device_inner(msh_serial, device_node);
            Ok(())
        }
        .boxed()
    }

    /// Removes a Meshtastic serial device, cancelling its background task via its drop guard.
    fn remove_device(&mut self, device_node: &str) -> bool {
        self.devices.remove(device_node).is_some()
    }

    /// Checks if a connection background task is currently running for the given device node.
    fn is_running(&self, device_node: &str) -> bool {
        self.devices.contains_key(device_node)
    }

    /// Name
    fn name(&self) -> &'static str {
        "Meshtastic"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::FakeMeshtasticSerial;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_meshtastic_factory_management() {
        let (mut factory, _mesh_stream) = MeshtasticFactory::new();

        let (_rx_tx, rx_rx) = mpsc::channel(1);
        let fake_serial = FakeMeshtasticSerial::new(MacAddress::default(), rx_rx);

        assert!(!factory.is_running("/dev/ttyUSB0"));

        factory.add_meshtastic_device_inner(fake_serial, "/dev/ttyUSB0");
        assert!(factory.is_running("/dev/ttyUSB0"));

        let removed = factory.remove_device("/dev/ttyUSB0");
        assert!(removed);
        assert!(!factory.is_running("/dev/ttyUSB0"));

        let removed_again = factory.remove_device("/dev/ttyUSB0");
        assert!(!removed_again);
    }

    #[test]
    fn test_detect_device_inner() {
        use serialport::{SerialPortInfo, SerialPortType, UsbPortInfo};

        let usb_info = UsbPortInfo {
            vid: 0x1234,
            pid: 0x5678,
            serial_number: Some("12345".to_owned()),
            manufacturer: None,
            product: None,
        };

        let mut port = SerialPortInfo {
            port_name: "/dev/ttyACM0".to_owned(),
            port_type: SerialPortType::UsbPort(usb_info.clone()),
        };

        // Match
        assert!(MeshtasticFactory::detect_device_inner(Some("12345"), &port));

        // Mismatch serial
        assert!(!MeshtasticFactory::detect_device_inner(
            Some("54321"),
            &port
        ));

        // Mismatch name
        port.port_name = "/dev/ttyUSB0".to_owned();
        assert!(!MeshtasticFactory::detect_device_inner(
            Some("12345"),
            &port
        ));

        // Match COM port
        port.port_name = "COM3".to_owned();
        assert!(MeshtasticFactory::detect_device_inner(Some("12345"), &port));

        // Not USB
        port.port_type = SerialPortType::PciPort;
        assert!(!MeshtasticFactory::detect_device_inner(
            Some("12345"),
            &port
        ));
    }
}
