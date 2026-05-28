use futures::StreamExt;
use log::{error, info};
use meshtastic::protobufs::MeshPacket;
use nusb::hotplug::HotplugEvent;
use serialport::SerialPortType;
use std::collections::{HashMap, hash_map::Entry};
use std::fmt::Display;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::future::FutureExt;
use tokio_util::sync::CancellationToken;

use crate::meshtastic_serial::MeshtasticSerial;
use crate::si_uart::TokioSerial;
use crate::system_info::MacAddress;
use yaroc_common::punch::RawPunch;
use yaroc_common::si_uart::SiUart;

pub trait UsbSerialTrait {
    type Output;

    /// An inner loop that reads messages from the serial device and sends them to a channel.
    fn inner_loop(self, tx: UnboundedSender<Self::Output>) -> impl Future<Output = ()> + Send;
}

/// Serial device manager
///
/// Handles connecting and disconnecting of serial devices (Meshtastic and SportIdent).
pub struct UsbSerialManager {
    cancellation_tokens: HashMap<String, CancellationToken>,
    mesh_tx: Option<UnboundedSender<(MeshPacket, MacAddress)>>,
    si_tx: Option<UnboundedSender<RawPunch>>,
    enable_meshtastic: bool,
    enable_sportident: bool,
}

const SI_LABS: u16 = 0x10c4;

impl UsbSerialManager {
    /// Creates a new `SerialDeviceManager`.
    pub fn new(
        mesh_tx: Option<UnboundedSender<(MeshPacket, MacAddress)>>,
        si_tx: Option<UnboundedSender<RawPunch>>,
    ) -> Self {
        Self {
            cancellation_tokens: HashMap::new(),
            enable_meshtastic: mesh_tx.is_some(),
            enable_sportident: si_tx.is_some(),
            mesh_tx,
            si_tx,
        }
    }

    /// Connects to a Meshtastic serial device.
    pub fn add_meshtastic_device_inner<M>(&mut self, msh_serial: M, device_node: &str)
    where
        M: UsbSerialTrait<Output = (MeshPacket, MacAddress)> + Send + Display + 'static,
    {
        if let Some(tx) = &self.mesh_tx {
            let token = self.spawn_serial(msh_serial, tx.clone());
            self.cancellation_tokens.insert(device_node.to_owned(), token);
        }
    }

    /// Adds a new Meshtastic device to be managed.
    pub async fn add_meshtastic_device(
        &mut self,
        port: &str,
        device_node: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let msh_serial = MeshtasticSerial::new(port, device_node, Duration::from_secs(12))
            .await
            .map_err(|err| -> Box<dyn std::error::Error + Send + Sync> {
                err.to_string().into()
            })?;
        let mac_address = msh_serial.mac_address();
        self.add_meshtastic_device_inner(msh_serial, device_node);
        info!("Connected to Meshtastic device: {mac_address} at {port}");
        Ok(())
    }

    /// Connects to a SportIdent serial device.
    pub fn add_sportident_device_inner<M>(&mut self, si_uart: M, device_node: &str)
    where
        M: UsbSerialTrait<Output = RawPunch> + Send + Display + 'static,
    {
        if let Some(tx) = &self.si_tx {
            let token = self.spawn_serial(si_uart, tx.clone());
            self.cancellation_tokens.insert(device_node.to_owned(), token);
        }
    }

    /// Adds a new SI UART device to be managed.
    pub fn add_sportident_device(
        &mut self,
        port: &str,
        device_node: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let serial = TokioSerial::new(port)?;
        let si_uart = SiUart::new(serial);
        self.add_sportident_device_inner(si_uart, device_node);
        info!("Connected to SI UART device at {port}");
        Ok(())
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
            occupied_entry.get().cancel();
            occupied_entry.remove();
            true
        } else {
            false
        }
    }

    /// Spawns a task to read messages from a serial connection.
    fn spawn_serial<M>(&self, usb_serial: M, tx: UnboundedSender<M::Output>) -> CancellationToken
    where
        M: UsbSerialTrait + Send + Display + 'static,
        M::Output: Send,
    {
        let cancellation_token = CancellationToken::new();
        let cancellation_token_clone = cancellation_token.clone();
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

    /// Monitors USB hotplug events and manages devices dynamically.
    pub async fn monitor_usb_devices(
        &mut self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // TODO: print whether we are monitoring meshtastic and Sportident
        info!("Starting USB device manager");
        if let Ok(devices) = nusb::list_devices().await {
            for dev in devices {
                self.detect_device(&dev).await;
            }
        }

        let mut watcher = nusb::watch_devices()?;
        while let Some(event) = watcher.next().await {
            match event {
                HotplugEvent::Connected(dev) => {
                    // Give the OS TTY subsystem a brief moment to register the node
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    self.detect_device(&dev).await;
                }
                HotplugEvent::Disconnected(dev_id) => {
                    let device_node = format!("{:?}", dev_id);
                    if self.remove_device(device_node.clone()) {
                        info!("Disconnected USB device {device_node}");
                    }
                }
            }
        }

        Ok(())
    }

    /// Detects the serial port name, device ID, and type for a given USB device.
    async fn detect_device(&mut self, dev: &nusb::DeviceInfo) {
        let Ok(ports) = serialport::available_ports() else {
            return;
        };
        for port in ports {
            if let SerialPortType::UsbPort(usb_info) = &port.port_type
                && usb_info.vid == dev.vendor_id()
            {
                let sn_matches = match (dev.serial_number(), &usb_info.serial_number) {
                    (Some(dev_serial_n), Some(usb_serial_n)) => dev_serial_n == usb_serial_n,
                    (None, None) => true,
                    _ => false,
                };
                if !sn_matches {
                    return;
                }
                if usb_info.vid == SI_LABS && self.enable_sportident {
                    let _ = self
                        .add_sportident_device(&port.port_name, &format!("{:?}", dev.id()))
                        .inspect_err(|err| {
                            error!(
                                "Failed to connect to SI UART device at {}: {err}",
                                port.port_name
                            )
                        });
                } else if port.port_name.contains("ACM") || port.port_name.contains("COM") {
                    if !self.enable_meshtastic {
                        return;
                    }
                    let _ = self
                        .add_meshtastic_device(&port.port_name, &format!("{:?}", dev.id()))
                        .await
                        .inspect_err(|err| {
                            error!(
                                "Failed to connect to Meshtastic device at {}: {err}",
                                port.port_name
                            )
                        });
                }
            }
        }
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
        let mut handler = UsbSerialManager::new(Some(proto_tx), None);
        handler.add_meshtastic_device_inner(fake_serial, "/some");

        let (recv_packet, recv_mac) = proto_rx.recv().await.unwrap();
        assert_eq!(recv_mac, Default::default());
        assert_eq!(recv_packet, packet);

        handler.remove_device("/some".to_owned());
        assert!(!handler.is_running("/some"));
    }
}
