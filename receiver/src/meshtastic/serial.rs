use std::collections::HashMap;
use std::fmt::Display;
use std::time::Duration;

use futures::FutureExt as _;
use futures::future::BoxFuture;
use log::info;
use meshtastic::protobufs::ServiceEnvelope;
use meshtastic::utils::stream;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use super::connection::MeshtasticConnection;
pub use super::connection::MeshtasticEvent;
use crate::si_uart::SILABS_VID;
use crate::system_info::MacAddress;
use crate::usb_serial_manager::{UsbSerialFactory, UsbSerialTrait};

/// A connection to a Meshtastic device.
pub struct MeshtasticSerial {
    port: String,
    connection: MeshtasticConnection,
}

impl MeshtasticSerial {
    /// Creates a new Meshtastic serial connection.
    pub async fn new(
        port: &str,
        timeout: Duration,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let serial_stream = stream::build_serial_stream(port.to_owned(), None, None, None)?;
        let connection = MeshtasticConnection::connect_stream(serial_stream, timeout).await?;
        info!(
            "Connected to Meshtastic device: {} at {port}",
            connection.mac_address
        );

        Ok(Self {
            port: port.to_owned(),
            connection,
        })
    }

    /// Returns the MAC address of the device.
    pub fn mac_address(&self) -> MacAddress {
        self.connection.mac_address
    }
}

impl UsbSerialTrait for MeshtasticSerial {
    type Output = ServiceEnvelope;

    /// An inner loop that reads messages from the Meshtastic device and sends them to a channel.
    async fn inner_loop(self, mesh_packet_tx: UnboundedSender<ServiceEnvelope>) {
        self.connection.inner_loop(mesh_packet_tx, &self.port).await;
    }
}

impl Display for MeshtasticSerial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Meshtastic serial device {}", self.mac_address())
    }
}

/// A factory for creating and managing background tasks for Meshtastic serial devices.
pub struct MeshtasticFactory {
    devices: HashMap<String, CancellationToken>,
    mesh_tx: UnboundedSender<ServiceEnvelope>,
}

impl MeshtasticFactory {
    /// Creates a new `MeshtasticFactory` that forwards mesh packets through the given sender.
    pub fn new(mesh_tx: UnboundedSender<ServiceEnvelope>) -> Self {
        Self {
            devices: HashMap::new(),
            mesh_tx,
        }
    }

    /// Spawns the serial reading background loop for the given Meshtastic serial device and
    /// registers its cancellation token.
    pub fn add_meshtastic_device_inner<M>(&mut self, msh_serial: M, device_node: &str)
    where
        M: UsbSerialTrait<Output = ServiceEnvelope> + Send + Display + 'static,
    {
        let token = msh_serial.spawn_serial(self.mesh_tx.clone());
        self.devices.insert(device_node.to_owned(), token);
    }

    /// Inner logic for device detection that can be unit tested without a `nusb::DeviceInfo`.
    fn detect_device_inner(dev_serial: Option<&str>, port: &serialport::SerialPortInfo) -> bool {
        if let serialport::SerialPortType::UsbPort(usb_info) = &port.port_type {
            let sn_matches = match (dev_serial, &usb_info.serial_number) {
                (Some(dev_serial_n), Some(usb_serial_n)) => dev_serial_n == usb_serial_n,
                (None, None) => true,
                _ => false,
            };
            let is_sportident = usb_info.vid == SILABS_VID;
            sn_matches
                && !is_sportident
                && (port.port_name.contains("ACM") || port.port_name.contains("COM"))
        } else {
            false
        }
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

    /// Removes a Meshtastic serial device by triggering its background task cancellation.
    fn remove_device(&mut self, device_node: &str) -> bool {
        if let Some(token) = self.devices.remove(device_node) {
            token.cancel();
            true
        } else {
            false
        }
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
        let (mesh_tx, mut _mesh_rx) = mpsc::unbounded_channel();
        let mut factory = MeshtasticFactory::new(mesh_tx);

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
