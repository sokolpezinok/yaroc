use futures::StreamExt;
use futures::future::BoxFuture;
use log::{debug, error, info};
use meshtastic::protobufs::ServiceEnvelope;
use nusb::hotplug::HotplugEvent;
use std::fmt::Display;
#[cfg(all(target_os = "linux", feature = "udev"))]
use std::path::PathBuf;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::future::FutureExt;
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::meshtastic::serial::MeshtasticFactory;
use crate::si_uart::{SportIdentFactory, SportIdentMessage};

pub trait UsbSerialTrait {
    type Output;

    /// Converts this serial device into a stream of messages.
    fn into_stream(self) -> impl futures::Stream<Item = Self::Output> + Send;

    /// Spawns a task to read messages from a serial connection.
    fn spawn_serial(self, tx: UnboundedSender<Self::Output>) -> DropGuard
    where
        Self: Sized + Send + Display + 'static,
        Self::Output: Send,
    {
        let cancellation_token = CancellationToken::new();
        let drop_guard = cancellation_token.clone().drop_guard();
        tokio::spawn(async move {
            let description = format!("{self}");
            let description_ref = &description;
            let fut = async move {
                let mut stream = std::pin::pin!(self.into_stream());
                while let Some(item) = stream.next().await {
                    if let Err(err) = tx.send(item) {
                        error!("Failed to forward message from {description_ref}: {err}");
                        break;
                    }
                }
            };
            let res = fut.with_cancellation_token_owned(cancellation_token).await;
            if res.is_none() {
                info!("Stopping {}", description);
            }
        });

        drop_guard
    }
}

pub trait UsbSerialFactory: Send + Sync {
    fn detect_device(&self, dev: &nusb::DeviceInfo, port: &serialport::SerialPortInfo) -> bool;

    fn add_device<'a>(
        &'a mut self,
        port: &'a str,
        device_node: &'a str,
    ) -> BoxFuture<'a, Result<(), Box<dyn std::error::Error + Send + Sync>>>;

    fn remove_device(&mut self, device_node: &str) -> bool;

    fn is_running(&self, device_node: &str) -> bool;

    fn name(&self) -> &'static str;
}

#[derive(Default)]
pub enum SportIdentConfig {
    #[default]
    None,
    Passive,
    Active(Box<dyn UsbSerialFactory>),
}

#[derive(Default)]
pub struct UsbSerialConfig {
    pub enable_meshtastic: bool,
    pub sportident: SportIdentConfig,
}

/// Serial device manager
///
/// Handles connecting and disconnecting of serial devices (Meshtastic and SportIdent).
pub struct UsbSerialManager {
    factories: Vec<Box<dyn UsbSerialFactory>>,
    #[cfg(all(target_os = "linux", feature = "udev"))]
    pending_connections: Vec<nusb::DeviceInfo>,
}

#[cfg(all(target_os = "linux", feature = "udev"))]
struct UdevEvent {
    devnode: PathBuf,
    usb_sysfs_path: PathBuf,
}

#[cfg(all(target_os = "linux", feature = "udev"))]
fn extract_add_event(event: tokio_udev::Event) -> Option<UdevEvent> {
    if event.event_type() != tokio_udev::EventType::Add {
        return None;
    }
    let devnode = event.devnode()?;
    let mut parent = event.parent();
    while let Some(p) = parent {
        if p.subsystem().is_some_and(|s| s == "usb")
            && p.devtype().is_some_and(|s| s == "usb_device")
        {
            return Some(UdevEvent {
                devnode: devnode.to_path_buf(),
                usb_sysfs_path: p.syspath().to_path_buf(),
            });
        }
        parent = p.parent();
    }
    None
}

impl UsbSerialManager {
    /// Creates a new `SerialDeviceManager`.
    pub fn new(factories: Vec<Box<dyn UsbSerialFactory>>) -> Self {
        Self {
            factories,
            #[cfg(all(target_os = "linux", feature = "udev"))]
            pending_connections: Vec::new(),
        }
    }

    /// Creates a `UsbSerialManager` and optional Meshtastic and SportIdent streams from a [`UsbSerialConfig`].
    pub fn from_config(
        config: UsbSerialConfig,
    ) -> (
        Option<Self>,
        Option<impl futures::Stream<Item = ServiceEnvelope> + Send>,
        Option<impl futures::Stream<Item = SportIdentMessage> + Send>,
    ) {
        let mut factories: Vec<Box<dyn UsbSerialFactory>> = Vec::new();
        let mesh_stream = if config.enable_meshtastic {
            let (factory, stream) = MeshtasticFactory::new();
            factories.push(Box::new(factory));
            Some(stream)
        } else {
            None
        };
        let si_stream = match config.sportident {
            SportIdentConfig::Passive => {
                let (factory, stream) = SportIdentFactory::new();
                factories.push(Box::new(factory));
                Some(stream)
            }
            SportIdentConfig::Active(factory) => {
                factories.push(factory);
                None
            }
            SportIdentConfig::None => None,
        };
        let manager = if factories.is_empty() {
            None
        } else {
            Some(Self::new(factories))
        };
        (manager, mesh_stream, si_stream)
    }

    /// Indicates whether the device is connected and running.
    pub fn is_running(&self, device_node: &str) -> bool {
        self.factories.iter().any(|f| f.is_running(device_node))
    }

    /// Disconnects a serial device.
    ///
    /// This function cancels the task that handles messages from the device and returns true if
    /// the device was connected.
    pub fn remove_device(&mut self, device_node: &str) -> bool {
        self.factories.iter_mut().any(|f| f.remove_device(device_node))
    }

    /// Spawns a background task to monitor USB hotplug events and returns a `DropGuard` that
    /// cancels the task when dropped.
    pub fn spawn(mut self) -> DropGuard {
        let cancellation_token = CancellationToken::new();
        let drop_guard = cancellation_token.clone().drop_guard();
        tokio::spawn(async move {
            let fut = async move {
                if let Err(e) = self.monitor_usb_devices().await {
                    error!("USB device manager failed: {e}");
                }
            };
            if fut.with_cancellation_token_owned(cancellation_token).await.is_none() {
                info!("Stopping USB device manager");
            }
        });
        drop_guard
    }

    #[cfg(all(target_os = "linux", feature = "udev"))]
    fn handle_hotplug_event(&mut self, event: HotplugEvent) {
        match event {
            HotplugEvent::Connected(dev) => {
                debug!(
                    "USB device connected (pending TTY subsystem): {:?}",
                    dev.sysfs_path()
                );
                let dev_id_str = format!("{:?}", dev.id());
                if !self.is_running(&dev_id_str) {
                    self.pending_connections.push(dev);
                }
            }
            HotplugEvent::Disconnected(dev_id) => {
                let device_node = format!("{:?}", dev_id);
                if self.remove_device(&device_node) {
                    debug!("Disconnected USB device {device_node}");
                }
                self.pending_connections.retain(|d| d.id() != dev_id);
            }
        }
    }

    #[cfg(all(target_os = "linux", feature = "udev"))]
    /// Monitors USB hotplug events and manages devices dynamically using udev.
    pub async fn monitor_usb_devices(
        &mut self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting USB device manager (Linux udev version)");
        if let Ok(devices) = nusb::list_devices().await {
            for dev in devices {
                self.detect_device(&dev).await;
            }
        }

        let udev_monitor = tokio_udev::MonitorBuilder::new()?.match_subsystem("tty")?.listen()?;
        let mut udev_stream =
            tokio_udev::AsyncMonitorSocket::new(udev_monitor)?.filter_map(|res| {
                futures::future::ready(
                    res.inspect_err(|err| error!("Udev stream error: {err:?}"))
                        .ok()
                        .and_then(extract_add_event),
                )
            });

        let mut watcher = nusb::watch_devices()?;
        loop {
            tokio::select! {
                Some(event) = watcher.next() => self.handle_hotplug_event(event),
                Some(udev_event) = udev_stream.next() => {
                    debug!("TTY subsystem device added: {:?}", udev_event.devnode);
                    if let Some(idx) = self
                        .pending_connections
                        .iter()
                        .position(|dev| dev.sysfs_path() == udev_event.usb_sysfs_path)
                    {
                        let dev = self.pending_connections.remove(idx);
                        self.detect_device(&dev).await;
                    }
                }
                else => {
                    error!("USB or udev monitor stream ended unexpectedly");
                    break;
                }
            }
        }
        Ok(())
    }

    #[cfg(not(all(target_os = "linux", feature = "udev")))]
    /// Monitors USB hotplug events and manages devices dynamically.
    pub async fn monitor_usb_devices(
        &mut self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    self.detect_device(&dev).await;
                }
                HotplugEvent::Disconnected(dev_id) => {
                    let device_node = format!("{:?}", dev_id);
                    if self.remove_device(&device_node) {
                        debug!("Disconnected USB device {device_node}");
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
        let dev_id = format!("{:?}", dev.id());
        for port in ports {
            for factory in &mut self.factories {
                if factory.detect_device(dev, &port) {
                    let _ = factory.add_device(&port.port_name, &dev_id).await.inspect_err(|err| {
                        error!(
                            "Failed to connect to {} device at {}: {err}",
                            factory.name(),
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
    use crate::system_info::MacAddress;
    use meshtastic::protobufs::MeshPacket;
    use tokio::sync::mpsc;

    use crate::test_utils::FakeMeshtasticSerial;

    #[tokio::test]
    async fn test_meshtastic_serial() {
        let (mut factory, proto_stream) = MeshtasticFactory::new();
        let mut proto_stream = std::pin::pin!(proto_stream);
        let (tx, rx) = mpsc::channel(1);
        let fake_serial = FakeMeshtasticSerial::new(MacAddress::default(), rx);
        factory.add_meshtastic_device_inner(fake_serial, "/some");
        let mut handler = UsbSerialManager::new(vec![Box::new(factory)]);

        let packet = MeshPacket {
            from: 0x1234,
            to: 0xabcd,
            ..Default::default()
        };
        tx.send(packet.clone()).await.unwrap();
        let recv_envelope = proto_stream.next().await.unwrap();
        let gateway_id =
            recv_envelope.gateway_id.strip_prefix('!').unwrap_or(&recv_envelope.gateway_id);
        let recv_mac = MacAddress::try_from(gateway_id).unwrap();
        assert_eq!(recv_mac, MacAddress::default());
        assert_eq!(recv_envelope.packet.unwrap(), packet);

        handler.remove_device("/some");
        assert!(!handler.is_running("/some"));
    }
}
