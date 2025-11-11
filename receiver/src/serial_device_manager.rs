use std::collections::{HashMap, hash_map::Entry};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

pub trait UsbSerialTrait {
    type Output;

    /// An inner loop that reads messages from the serial device and sends them to a channel.
    fn inner_loop(
        self,
        cancellation_token: CancellationToken,
        tx: UnboundedSender<Self::Output>,
    ) -> impl Future<Output = ()> + Send;
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
    pub fn add_device(&mut self, msh_serial: M, device_node: &str) {
        let token = self.spawn_serial(msh_serial);
        self.cancellation_tokens.insert(device_node.to_owned(), token);
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
    fn spawn_serial(&mut self, meshtastic_serial: M) -> CancellationToken {
        let cancellation_token = CancellationToken::new();
        let cancellation_token_clone = cancellation_token.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move { meshtastic_serial.inner_loop(cancellation_token, tx).await });

        cancellation_token_clone
    }

    #[cfg(test)]
    fn is_running(&self, device_node: &str) -> bool {
        self.cancellation_tokens.contains_key(device_node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{meshtastic_serial::MeshtasticEvent, system_info::MacAddress};
    use futures::Future;
    use meshtastic::protobufs::MeshPacket;
    use tokio::sync::mpsc::{self, Receiver, UnboundedSender};
    use tokio_util::sync::CancellationToken;

    pub struct FakeMeshtasticSerial {
        mac_address: MacAddress,
        rx: Receiver<MeshPacket>,
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
        fn inner_loop(
            mut self,
            cancellation_token: CancellationToken,
            mesh_proto_tx: UnboundedSender<(MeshPacket, MacAddress)>,
        ) -> impl Future<Output = ()> + Send {
            let mac_address = self.mac_address;
            async move {
                loop {
                    tokio::select! {
                        _ = cancellation_token.cancelled() => {
                            break;
                        }
                        event = self.next_message() => {
                            match event {
                                MeshtasticEvent::MeshPacket(mesh_packet) => {
                                    mesh_proto_tx
                                        .send((mesh_packet, mac_address))
                                        .expect("Channel unexpectedly closed");
                                }
                                MeshtasticEvent::Disconnected(_device_node) => {
                                    cancellation_token.cancel();
                                }
                            }
                        }
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
