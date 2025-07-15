use meshtastic::api::{ConnectedStreamApi, StreamApi};
use meshtastic::protobufs::{FromRadio, MeshPacket, from_radio};
use meshtastic::utils;
use tokio::sync::mpsc::UnboundedReceiver;

pub struct MeshtasticSerial {
    stream_api: ConnectedStreamApi,
    listener: UnboundedReceiver<FromRadio>,
}

impl MeshtasticSerial {
    pub async fn new(port: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let stream_api = StreamApi::new();
        let serial_stream = utils::stream::build_serial_stream(port.to_owned(), None, None, None)?;
        let (listener, stream_api) = stream_api.connect(serial_stream).await;

        let config_id = utils::generate_rand_id();
        let stream_api = stream_api.configure(config_id).await?;
        Ok(Self {
            stream_api,
            listener,
        })
    }

    pub async fn disconnect(self) -> Result<(), Box<dyn std::error::Error>> {
        self.stream_api.disconnect().await?;
        Ok(())
    }

    pub async fn next_message(&mut self) -> Option<MeshPacket> {
        loop {
            match self.listener.recv().await {
                Some(FromRadio {
                    payload_variant: Some(from_radio::PayloadVariant::Packet(packet)),
                    ..
                }) => {
                    return Some(packet);
                }
                None => {
                    return None;
                }
                _ => {}
            }
        }
    }
}
