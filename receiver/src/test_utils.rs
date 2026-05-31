use std::fmt::Display;
use tokio::sync::mpsc::{Receiver, UnboundedSender};

use crate::meshtastic_serial::MeshtasticEvent;
use crate::system_info::MacAddress;
use crate::usb_serial_manager::UsbSerialTrait;
use meshtastic::protobufs::MeshPacket;

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

    pub async fn next_message(&mut self) -> MeshtasticEvent {
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
