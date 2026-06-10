use futures::StreamExt;
use futures::future::BoxFuture;
use log::{debug, error, info};
use nusb::hotplug::HotplugEvent;
use std::fmt::Display;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::pin::Pin;
#[cfg(target_os = "linux")]
use std::task::{Context, Poll};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::future::FutureExt;
use tokio_util::sync::CancellationToken;
pub trait UsbSerialTrait {
    type Output;

    /// An inner loop that reads messages from the serial device and sends them to a channel.
    fn inner_loop(self, tx: UnboundedSender<Self::Output>) -> impl Future<Output = ()> + Send;

    /// Spawns a task to read messages from a serial connection.
    fn spawn_serial(self, tx: UnboundedSender<Self::Output>) -> CancellationToken
    where
        Self: Sized + Send + Display + 'static,
        Self::Output: Send,
    {
        let cancellation_token = CancellationToken::new();
        let cancellation_token_clone = cancellation_token.clone();
        tokio::spawn(async move {
            let description = format!("{self}");
            let res = self.inner_loop(tx).with_cancellation_token_owned(cancellation_token).await;
            if res.is_none() {
                info!("Stopping {}", description);
            }
        });

        cancellation_token_clone
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
}

/// Serial device manager
///
/// Handles connecting and disconnecting of serial devices (Meshtastic and SportIdent).
pub struct UsbSerialManager {
    factories: Vec<Box<dyn UsbSerialFactory>>,
    pending_connections: Vec<nusb::DeviceInfo>,
}

#[cfg(target_os = "linux")]
struct SendAsyncMonitorSocket(tokio_udev::AsyncMonitorSocket);
#[cfg(target_os = "linux")]
// SAFETY: tokio_udev::AsyncMonitorSocket wraps a raw file descriptor (netlink socket)
// which can be safely transferred to another thread. The underlying tokio async registration
// and standard system calls are thread-safe.
unsafe impl Send for SendAsyncMonitorSocket {}
#[cfg(target_os = "linux")]
// SAFETY: Access to the socket is synchronized via standard mut references and async polling
// in tokio, making concurrent access thread-safe.
unsafe impl Sync for SendAsyncMonitorSocket {}

#[cfg(target_os = "linux")]
impl futures::Stream for SendAsyncMonitorSocket {
    type Item = std::io::Result<tokio_udev::Event>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0).poll_next(cx)
    }
}

#[cfg(target_os = "linux")]
struct UdevEvent {
    devnode: PathBuf,
    usb_sysfs_path: PathBuf,
}

#[cfg(target_os = "linux")]
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
            pending_connections: Vec::new(),
        }
    }

    /// Indicates whether the device is connected and running.
    pub fn is_running(&self, device_node: &str) -> bool {
        self.factories.iter().any(|f| f.is_running(device_node))
    }

    /// Disconnects a serial device.
    ///
    /// This function cancels the task that handles messages from the device and returns true if
    /// the device was connected.
    pub fn remove_device(&mut self, device_node: String) -> bool {
        self.factories.iter_mut().any(|f| f.remove_device(&device_node))
    }

    #[cfg(target_os = "linux")]
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
                if self.remove_device(device_node.clone()) {
                    debug!("Disconnected USB device {device_node}");
                }
                self.pending_connections.retain(|d| d.id() != dev_id);
            }
        }
    }

    #[cfg(target_os = "linux")]
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
        let udev_stream =
            SendAsyncMonitorSocket(tokio_udev::AsyncMonitorSocket::new(udev_monitor)?);

        let mut udev_stream = udev_stream.filter_map(|res| {
            futures::future::ready(match res {
                Ok(event) => extract_add_event(event).map(Ok),
                Err(err) => Some(Err(err)),
            })
        });

        let mut watcher = nusb::watch_devices()?;
        loop {
            tokio::select! {
                event = watcher.next() => {
                    match event {
                        Some(event) => self.handle_hotplug_event(event),
                        None => {
                            error!("USB watcher stream ended unexpectedly");
                            break;
                        }
                    }
                }
                res = udev_stream.next() => {
                    match res {
                        Some(Ok(udev_event)) => {
                            let devnode = &udev_event.devnode;
                            debug!("TTY subsystem device added: {:?}", devnode);
                            let usb_path = &udev_event.usb_sysfs_path;
                            if let Some(idx) = self.pending_connections.iter().position(|dev| dev.sysfs_path() == usb_path) {
                                let dev = self.pending_connections.remove(idx);
                                self.detect_device(&dev).await;
                            }
                        }
                        Some(Err(err)) => {
                            error!("Udev stream error: {:?}", err);
                        }
                        None => {
                            error!("Udev monitor stream ended unexpectedly");
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
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
                    if self.remove_device(device_node.clone()) {
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
        for port in ports {
            for factory in &mut self.factories {
                if factory.detect_device(dev, &port) {
                    let _ = factory
                        .add_device(&port.port_name, &format!("{:?}", dev.id()))
                        .await
                        .inspect_err(|err| {
                            error!("Failed to connect to device at {}: {err}", port.port_name)
                        });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{meshtastic_serial::MeshtasticFactory, system_info::MacAddress};
    use meshtastic::protobufs::MeshPacket;
    use tokio::sync::mpsc;

    use crate::test_utils::FakeMeshtasticSerial;

    #[tokio::test]
    async fn test_meshtastic_serial() {
        let (proto_tx, mut proto_rx) = mpsc::unbounded_channel();
        let mut factory = MeshtasticFactory::new(proto_tx);
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
        let (recv_packet, recv_mac) = proto_rx.recv().await.unwrap();
        assert_eq!(recv_mac, MacAddress::default());
        assert_eq!(recv_packet, packet);

        handler.remove_device("/some".to_owned());
        assert!(!handler.is_running("/some"));
    }
}
