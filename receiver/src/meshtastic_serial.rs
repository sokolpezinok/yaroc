use std::collections::HashMap;
use std::fmt::Display;
use std::time::Duration;

use futures::FutureExt as _;
use futures::future::BoxFuture;
use log::{info, warn};
use meshtastic::api::{ConnectedStreamApi, StreamApi};
use meshtastic::protobufs::{FromRadio, MeshPacket, from_radio};
use meshtastic::utils;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;
use tokio_util::future::FutureExt;
use tokio_util::sync::CancellationToken;

use crate::error::Error;
use crate::si_uart::SILABS_VID;
use crate::system_info::MacAddress;
use crate::usb_serial_manager::{UsbSerialFactory, UsbSerialTrait};

/// An enum representing a message from a Meshtastic device.
pub enum MeshtasticEvent {
    /// A mesh packet.
    MeshPacket(MeshPacket),
    /// The device was disconnected.
    Disconnected(String),
}

/// A connection to a Meshtastic device.
pub struct MeshtasticSerial {
    device_node: String,
    stream_api: ConnectedStreamApi,
    listener: UnboundedReceiver<FromRadio>,
    mac_address: MacAddress,
}

impl MeshtasticSerial {
    /// Creates a new Meshtastic serial connection using a provided stream.
    pub async fn connect_stream<S>(
        stream: meshtastic::api::StreamHandle<S>,
        device_node: &str,
        timeout: Duration,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        let deadline = Instant::now() + timeout;
        let stream_api = StreamApi::new();
        let (mut listener, stream_api) = stream_api.connect(stream).timeout_at(deadline).await?;
        let config_id = utils::generate_rand_id();
        let stream_api = stream_api.configure(config_id).await?;
        let packet = listener.recv().timeout_at(deadline).await?;
        let Some(FromRadio {
            payload_variant: Some(from_radio::PayloadVariant::MyInfo(my_node_info)),
            ..
        }) = packet
        else {
            return Err(Box::new(Error::ConnectionError));
        };

        Ok(Self {
            device_node: device_node.to_owned(),
            stream_api,
            listener,
            mac_address: MacAddress::Meshtastic(my_node_info.my_node_num),
        })
    }

    /// Creates a new Meshtastic serial connection.
    pub async fn new(
        port: &str,
        device_node: &str,
        timeout: Duration,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let serial_stream = utils::stream::build_serial_stream(port.to_owned(), None, None, None)?;
        Self::connect_stream(serial_stream, device_node, timeout).await
    }

    /// Waits for the next message from the device.
    async fn next_message(&mut self) -> MeshtasticEvent {
        loop {
            match self.listener.recv().await {
                Some(FromRadio {
                    payload_variant: Some(from_radio::PayloadVariant::Packet(packet)),
                    ..
                }) => {
                    return MeshtasticEvent::MeshPacket(packet);
                }
                None => {
                    return MeshtasticEvent::Disconnected(self.device_node.clone());
                }
                _ => {}
            }
        }
    }

    /// Returns the device node.
    pub fn device_node(&self) -> &str {
        &self.device_node
    }

    /// Returns the MAC address of the device.
    pub fn mac_address(&self) -> MacAddress {
        self.mac_address
    }

    /// Disconnects the Meshtastic device.
    pub async fn disconnect(self) -> Result<(), Box<dyn std::error::Error>> {
        self.stream_api.disconnect().await?;
        Ok(())
    }
}

impl UsbSerialTrait for MeshtasticSerial {
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
                    warn!("Removed meshtastic device: {}", self.mac_address);
                    // Disconnect can return an error if the connection was already lost (e.g. EOF)
                    // We ignore it here as we are already handling the disconnection.
                    let _ = self.stream_api.disconnect().await;
                    break;
                }
            }
        }
    }
}

impl Display for MeshtasticSerial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Meshtastic device {}", self.mac_address)
    }
}

/// A factory for creating and managing background tasks for Meshtastic serial devices.
pub struct MeshtasticFactory {
    devices: HashMap<String, CancellationToken>,
    mesh_tx: UnboundedSender<(MeshPacket, MacAddress)>,
}

impl MeshtasticFactory {
    /// Creates a new `MeshtasticFactory` that forwards mesh packets through the given sender.
    pub fn new(mesh_tx: UnboundedSender<(MeshPacket, MacAddress)>) -> Self {
        Self {
            devices: HashMap::new(),
            mesh_tx,
        }
    }

    /// Spawns the serial reading background loop for the given Meshtastic serial device and
    /// registers its cancellation token.
    pub fn add_meshtastic_device_inner<M>(&mut self, msh_serial: M, device_node: &str)
    where
        M: UsbSerialTrait<Output = (MeshPacket, MacAddress)> + Send + Display + 'static,
    {
        let token = msh_serial.spawn_serial(self.mesh_tx.clone());
        self.devices.insert(device_node.to_owned(), token);
    }
}

impl UsbSerialFactory for MeshtasticFactory {
    /// Checks if a USB device matches a Meshtastic serial device by comparing serial numbers
    /// and ensuring the port name is an ACM/COM interface.
    fn detect_device(&self, dev: &nusb::DeviceInfo, port: &serialport::SerialPortInfo) -> bool {
        if let serialport::SerialPortType::UsbPort(usb_info) = &port.port_type {
            let sn_matches = match (dev.serial_number(), &usb_info.serial_number) {
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

    /// Asynchronously connects to a Meshtastic serial device at the given port, creates
    /// a new `MeshtasticSerial` instance, and adds it to the active device map.
    fn add_device<'a>(
        &'a mut self,
        port: &'a str,
        device_node: &'a str,
    ) -> BoxFuture<'a, Result<(), Box<dyn std::error::Error + Send + Sync>>> {
        async move {
            let msh_serial =
                MeshtasticSerial::new(port, device_node, std::time::Duration::from_secs(12))
                    .await
                    .map_err(|err| -> Box<dyn std::error::Error + Send + Sync> {
                        err.to_string().into()
                    })?;
            let mac_address = msh_serial.mac_address();
            self.add_meshtastic_device_inner(msh_serial, device_node);
            info!("Connected to Meshtastic device: {mac_address} at {port}");
            Ok(())
        }
        .boxed()
    }

    /// Removes a Meshtastic serial device by triggering its background task cancellation.
    fn remove_device(&mut self, device_node: &str) -> bool {
        if let Some(token) = self.devices.remove(device_node) {
            token.cancel();
            // TODO: add more info
            warn!("Removed meshtastic device");
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
