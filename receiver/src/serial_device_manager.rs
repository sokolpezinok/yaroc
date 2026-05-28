use log::{debug, error, info};
use std::collections::{HashMap, hash_map::Entry};
use std::fmt::Display;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::future::FutureExt;
use tokio_util::sync::CancellationToken;

use crate::si_uart::TokioSerial;
use nusb::hotplug::HotplugEvent;
use serialport::SerialPortType;
use std::time::Duration;
use yaroc_common::si_uart::SiUart;

pub trait UsbSerialTrait {
    type Output;

    /// An inner loop that reads messages from the serial device and sends them to a channel.
    fn inner_loop(self, tx: UnboundedSender<Self::Output>) -> impl Future<Output = ()> + Send;
}

/// Serial device manager
///
/// Handles connecting and disconnecting of serial devices. Supports only serial port
/// connections right now.
pub struct SerialDeviceManager<M: UsbSerialTrait + Send + 'static> {
    cancellation_tokens: HashMap<String, CancellationToken>,
    tx: UnboundedSender<M::Output>,
}

impl<M: UsbSerialTrait + Send + 'static> SerialDeviceManager<M>
where
    <M as UsbSerialTrait>::Output: Send,
    M: Display,
{
    /// Creates a new `SerialDeviceManager`.
    ///
    /// The handler is responsible for forwarding messages from the serial devices to the
    /// message handler.
    pub fn new(tx: UnboundedSender<M::Output>) -> Self {
        Self {
            cancellation_tokens: HashMap::new(),
            tx,
        }
    }

    /// Connects to a serial device.
    ///
    /// This function spawns a task to handle messages from the device.
    pub fn add_device(&mut self, usb_serial: M, device_node: &str) {
        let token = self.spawn_serial(usb_serial);
        self.cancellation_tokens.insert(device_node.to_owned(), token);
    }

    /// Indicates whether the device is connected and running.
    pub fn is_running(&self, device_node: &str) -> bool {
        self.cancellation_tokens.contains_key(device_node)
    }

    /// Disconnects a serial device.
    ///
    /// This function cancels the task that handles messages from the device and returns true if
    /// the device was connected.
    pub fn remove_device(&mut self, device_node: String) -> bool {
        if let Entry::Occupied(occupied_entry) = self.cancellation_tokens.entry(device_node) {
            // Note: the message in spawn_serial is logged first, but with a MAC address. We do not
            // log anything here.
            occupied_entry.get().cancel();
            occupied_entry.remove();
            true
        } else {
            false
        }
    }

    /// Spawns a task to read messages from a serial connection.
    ///
    /// The task forwards the messages to the message handler and can be cancelled by the returned
    /// `CancellationToken`.
    fn spawn_serial(&self, usb_serial: M) -> CancellationToken {
        let cancellation_token = CancellationToken::new();
        let cancellation_token_clone = cancellation_token.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let description = format!("{usb_serial}");

            let res = usb_serial
                .inner_loop(tx)
                .with_cancellation_token_owned(cancellation_token)
                .await;
            if res.is_none() {
                info!("Stopping {}", description);
            }
        });

        cancellation_token_clone
    }
}

const SI_LABS: u16 = 0x10c4;
type SiUartTokio = SiUart<TokioSerial>;

impl SerialDeviceManager<SiUartTokio> {
    /// Adds a new SI UART device to be managed.
    ///
    /// This method attempts to connect to the specified serial port and, upon successful connection,
    /// registers the device with the `SerialDeviceManager`.
    ///
    /// # Arguments
    /// * `port` - The serial port name (e.g., "/dev/ttyUSB0" or "COM1").
    /// * `device_node` - A unique identifier for the device.
    ///
    /// # Returns
    /// A `Result` indicating success or an error if the connection fails.
    pub fn add_device_inner(
        &mut self,
        port: &str,
        device_node: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let serial = TokioSerial::new(port)?;
        let si_uart = SiUart::new(serial);
        self.add_device(si_uart, device_node);
        info!("Connected to SI UART device at {port}");
        Ok(())
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
    pub fn detect_port(dev: &nusb::DeviceInfo) -> Option<(String, String)> {
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
    /// # Returns
    /// A `Result` that is `Ok(())` on successful monitoring loop termination, or an error if
    /// the hotplug watcher fails to initialize.
    pub async fn monitor_usb_devices(
        &mut self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting USB SportIdent device manager");
        if let Ok(devices) = nusb::list_devices().await {
            for dev in devices {
                if let Some((port_name, dev_id)) = Self::detect_port(&dev) {
                    let _ = self.add_device_inner(&port_name, &dev_id).inspect_err(|err| {
                        error!("Failed to connect to SI UART device at {port_name}: {err}")
                    });
                }
            }
        }

        let mut watcher = nusb::watch_devices()?;
        use futures::StreamExt;
        while let Some(event) = watcher.next().await {
            match event {
                HotplugEvent::Connected(dev) => {
                    // Give the OS TTY subsystem a brief moment to register the node
                    // TODO: is there a way to avaoid this sleep? Listen to tty subsystem events?
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    if let Some((port_name, dev_id)) = Self::detect_port(&dev) {
                        let _ = self.add_device_inner(&port_name, &dev_id).inspect_err(|err| {
                            error!("Failed to connect to SI UART device at {port_name}: {err}")
                        });
                    }
                }
                HotplugEvent::Disconnected(dev_id) => {
                    self.remove_device(format!("{:?}", dev_id));
                    info!("Disconnected SI UART device {dev_id:?}");
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{meshtastic_serial::MeshtasticEvent, system_info::MacAddress};
    use meshtastic::protobufs::MeshPacket;
    use tokio::sync::mpsc::{self, Receiver, UnboundedSender};

    pub struct FakeMeshtasticSerial {
        mac_address: MacAddress,
        rx: Receiver<MeshPacket>,
    }

    impl Display for FakeMeshtasticSerial {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "fake meshtastic serial")
        }
    }

    impl FakeMeshtasticSerial {
        pub fn new(mac_address: MacAddress, rx: Receiver<MeshPacket>) -> Self {
            Self { mac_address, rx }
        }

        async fn next_message(&mut self) -> MeshtasticEvent {
            let packet = self.rx.recv().await;
            match packet {
                Some(pkt) => MeshtasticEvent::MeshPacket(pkt),
                None => MeshtasticEvent::Disconnected("Fake".to_owned()),
            }
        }
    }

    impl UsbSerialTrait for FakeMeshtasticSerial {
        type Output = (MeshPacket, MacAddress);

        /// An inner loop that reads messages from the Meshtastic device and sends them to a channel.
        async fn inner_loop(mut self, mesh_proto_tx: UnboundedSender<(MeshPacket, MacAddress)>) {
            loop {
                let event = self.next_message().await;
                match event {
                    MeshtasticEvent::MeshPacket(mesh_packet) => {
                        mesh_proto_tx
                            .send((mesh_packet, self.mac_address))
                            .expect("Channel unexpectedly closed");
                    }
                    MeshtasticEvent::Disconnected(_device_node) => {
                        break;
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn test_meshtastic_serial() {
        let (tx, rx) = mpsc::channel(1);
        let fake_serial = FakeMeshtasticSerial::new(MacAddress::default(), rx);

        let packet = MeshPacket {
            from: 0x1234,
            to: 0xabcd,
            ..Default::default()
        };
        tx.send(packet.clone()).await.unwrap();
        let (proto_tx, mut proto_rx) = mpsc::unbounded_channel();
        let mut handler = SerialDeviceManager::new(proto_tx);
        handler.add_device(fake_serial, "/some");

        let (recv_packet, recv_mac) = proto_rx.recv().await.unwrap();
        assert_eq!(recv_mac, Default::default());
        assert_eq!(recv_packet, packet);

        handler.remove_device("/some".to_owned());
        assert!(!handler.is_running("/some"));
    }
}
