use log::{error, info, warn};
use meshtastic::api::{ConnectedStreamApi, StreamApi};
use meshtastic::protobufs::{FromRadio, MeshPacket, from_radio};
use meshtastic::utils;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::Instant;
use tokio_util::future::FutureExt as _;

use crate::error::Error;
use crate::system_info::MacAddress;

pub struct MeshtasticTcp {
    host: String,
    stream_api: ConnectedStreamApi,
    listener: tokio::sync::mpsc::UnboundedReceiver<FromRadio>,
    mac_address: MacAddress,
}

impl MeshtasticTcp {
    /// Connects to a Meshtastic device over TCP.
    pub async fn connect(
        host: &str,
        timeout: Duration,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let tcp_stream = utils::stream::build_tcp_stream(host.to_owned()).await.map_err(
            |err| -> Box<dyn std::error::Error + Send + Sync> { err.to_string().into() },
        )?;

        let deadline = Instant::now() + timeout;
        let stream_api = StreamApi::new();
        let (mut listener, stream_api) =
            stream_api.connect(tcp_stream).timeout_at(deadline).await?;

        let config_id = utils::generate_rand_id();
        let stream_api = stream_api.configure(config_id).await?;

        let packet = listener.recv().timeout_at(deadline).await?.ok_or_else(
            || -> Box<dyn std::error::Error + Send + Sync> {
                "Stream closed before configuration".into()
            },
        )?;
        let FromRadio {
            payload_variant: Some(from_radio::PayloadVariant::MyInfo(my_node_info)),
            ..
        } = packet
        else {
            return Err(Box::new(Error::ConnectionError));
        };

        Ok(Self {
            host: host.to_owned(),
            stream_api,
            listener,
            mac_address: MacAddress::Meshtastic(my_node_info.my_node_num),
        })
    }

    /// Returns the MAC address of the device.
    pub fn mac_address(&self) -> MacAddress {
        self.mac_address
    }

    /// An inner loop that reads messages from the Meshtastic device and sends them to a channel.
    pub async fn inner_loop(mut self, mesh_proto_tx: UnboundedSender<(MeshPacket, MacAddress)>) {
        info!(
            "Connected to Meshtastic TCP device: {} at {}",
            self.mac_address, self.host
        );
        loop {
            match self.listener.recv().await {
                Some(FromRadio {
                    payload_variant: Some(from_radio::PayloadVariant::Packet(packet)),
                    ..
                }) => {
                    if let Err(err) = mesh_proto_tx.send((packet, self.mac_address)) {
                        error!("Failed to forward packet: {err}");
                        break;
                    }
                }
                None => {
                    warn!("Disconnected from Meshtastic TCP device: {}", self.host);
                    let _ = self.stream_api.disconnect().await;
                    break;
                }
                _ => {}
            }
        }
    }
}
