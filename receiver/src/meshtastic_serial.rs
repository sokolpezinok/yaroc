use std::time::Duration;

use meshtastic::api::{ConnectedStreamApi, StreamApi};
use meshtastic::protobufs::{FromRadio, MeshPacket, MyNodeInfo, from_radio};
use meshtastic::utils;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{Instant, timeout_at};

use crate::error::Error;

pub struct MeshtasticSerial {
    device_node: String,
    stream_api: ConnectedStreamApi,
    listener: UnboundedReceiver<FromRadio>,
    node_num: u32,
}

pub enum MeshProto {
    MeshPacket(MeshPacket),
    MyNodeInfo(MyNodeInfo),
    Disconnected,
}

impl MeshtasticSerial {
    pub async fn new(
        port: &str,
        device_node: &str,
        timeout: Duration,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + timeout;
        let stream_api = StreamApi::new();
        let serial_stream = utils::stream::build_serial_stream(port.to_owned(), None, None, None)?;
        let (mut listener, stream_api) =
            timeout_at(deadline, stream_api.connect(serial_stream)).await?;
        let config_id = utils::generate_rand_id();
        let stream_api = stream_api.configure(config_id).await?;

        let packet = timeout_at(deadline, listener.recv()).await?;
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
            node_num: my_node_info.my_node_num,
        })
    }

    pub fn device_node(&self) -> &str {
        &self.device_node
    }

    pub async fn disconnect(self) -> Result<(), Box<dyn std::error::Error>> {
        self.stream_api.disconnect().await?;
        Ok(())
    }

    pub fn node_num(&self) -> u32 {
        self.node_num
    }

    pub async fn next_message(&mut self) -> MeshProto {
        loop {
            match self.listener.recv().await {
                Some(FromRadio {
                    payload_variant: Some(from_radio::PayloadVariant::Packet(packet)),
                    ..
                }) => {
                    return MeshProto::MeshPacket(packet);
                }
                Some(FromRadio {
                    payload_variant: Some(from_radio::PayloadVariant::MyInfo(my_node_info)),
                    ..
                }) => {
                    return MeshProto::MyNodeInfo(my_node_info);
                }
                None => {
                    return MeshProto::Disconnected;
                }
                _ => {}
            }
        }
    }
}
