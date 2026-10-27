use meshtastic::protobufs::{
    Data, FromRadio, MeshPacket, PortNum, ServiceEnvelope, mesh_packet::PayloadVariant,
};
use prost::Message;
use std::fmt::Display;
use tokio::sync::mpsc::Receiver;

use crate::meshtastic::connection::MeshtasticEvent;
use crate::si_uart::SportIdentMessage;
use crate::system_info::MacAddress;
use crate::usb_serial_manager::UsbSerialTrait;

/// Encodes a `FromRadio` protobuf message into a Meshtastic stream packet with magic header `[0x94, 0xc3]`.
pub fn encode_from_radio(msg: FromRadio) -> Vec<u8> {
    let mut protobuf_bytes = Vec::new();
    msg.encode(&mut protobuf_bytes).unwrap();
    let size = protobuf_bytes.len() as u16;
    let size_bytes = size.to_be_bytes();
    let mut header = vec![0x94, 0xc3, size_bytes[0], size_bytes[1]];
    header.extend_from_slice(&protobuf_bytes);
    header
}

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
            None => MeshtasticEvent::Disconnected,
        }
    }
}

impl UsbSerialTrait for FakeMeshtasticSerial {
    type Output = ServiceEnvelope;

    fn into_stream(self) -> impl futures::Stream<Item = ServiceEnvelope> + Send {
        futures::stream::unfold(self, |mut serial| async move {
            match serial.next_message().await {
                MeshtasticEvent::MeshPacket(mesh_packet) => {
                    let service_envelope = ServiceEnvelope {
                        packet: Some(mesh_packet),
                        channel_id: String::new(),
                        gateway_id: format!("!{}", serial.mac_address),
                    };
                    Some((service_envelope, serial))
                }
                MeshtasticEvent::Disconnected => None,
            }
        })
    }
}

pub struct FakeSportIdentSerial {
    port: String,
    rx: Receiver<SportIdentMessage>,
}

impl Display for FakeSportIdentSerial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "fake sportident serial at {}", self.port)
    }
}

impl FakeSportIdentSerial {
    pub fn new(port: String, rx: Receiver<SportIdentMessage>) -> Self {
        Self { port, rx }
    }

    pub async fn next_message(&mut self) -> Option<SportIdentMessage> {
        self.rx.recv().await
    }
}

impl UsbSerialTrait for FakeSportIdentSerial {
    type Output = SportIdentMessage;

    fn into_stream(self) -> impl futures::Stream<Item = SportIdentMessage> + Send {
        futures::stream::unfold(self, |mut serial| async move {
            serial.next_message().await.map(|msg| (msg, serial))
        })
    }
}

/// Helper to construct a `ServiceEnvelope` containing decoded data.
pub fn decoded_service_envelope(
    from: u32,
    portnum: PortNum,
    payload: Vec<u8>,
    gateway_id: impl Into<String>,
) -> ServiceEnvelope {
    ServiceEnvelope {
        packet: Some(MeshPacket {
            from,
            payload_variant: Some(PayloadVariant::Decoded(Data {
                portnum: portnum as i32,
                payload,
                ..Default::default()
            })),
            ..Default::default()
        }),
        gateway_id: gateway_id.into(),
        ..Default::default()
    }
}

/// Helper to construct a `ServiceEnvelope` containing encrypted data.
pub fn encrypted_service_envelope(
    from: u32,
    payload: Vec<u8>,
    gateway_id: impl Into<String>,
) -> ServiceEnvelope {
    ServiceEnvelope {
        packet: Some(MeshPacket {
            from,
            payload_variant: Some(PayloadVariant::Encrypted(payload)),
            ..Default::default()
        }),
        gateway_id: gateway_id.into(),
        ..Default::default()
    }
}
