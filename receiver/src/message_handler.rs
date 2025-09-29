use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::time::Duration;

use log::{error, info, warn};
use tokio::sync::mpsc::{Receiver, Sender, UnboundedReceiver, UnboundedSender, channel};
use tokio_util::sync::CancellationToken;

use crate::error::Error;
use crate::meshtastic_serial::{MeshProto, MeshtasticSerial};
use crate::mqtt::{MqttConfig, MqttReceiver};
use crate::state::{Event, FleetState};
use crate::system_info::MacAddress;

pub struct MshDevNotifier {
    dev_event_tx: Sender<MshDevEvent>,
}

pub enum MshDevEvent {
    DeviceAdded { port: String, device_node: String },
    DeviceRemoved { device_node: String },
}

pub struct MessageHandler {
    fleet_state: FleetState,
    mqtt_receiver: Option<MqttReceiver>,
    msh_dev_event_rx: Receiver<MshDevEvent>,
    msh_dev_event_tx: Sender<MshDevEvent>,
    mesh_proto_tx: UnboundedSender<(MeshProto, MacAddress)>,
    mesh_proto_rx: UnboundedReceiver<(MeshProto, MacAddress)>,
    cancellation_tokens: HashMap<String, CancellationToken>,
}

impl MshDevNotifier {
    pub fn add_device(&self, port: String, device_node: String) -> crate::Result<()> {
        self.dev_event_tx
            .try_send(MshDevEvent::DeviceAdded { port, device_node })
            .map_err(|_| Error::ChannelSendError)
    }

    pub fn remove_device(&self, device_node: String) -> crate::Result<()> {
        self.dev_event_tx
            .try_send(MshDevEvent::DeviceRemoved { device_node })
            .map_err(|_| Error::ChannelSendError)
    }
}

impl MessageHandler {
    pub fn new(dns: Vec<(String, MacAddress)>, mqtt_config: Option<MqttConfig>) -> Self {
        let macs = dns.iter().map(|(_, mac)| mac);
        let mqtt_receiver = mqtt_config.map(|config| MqttReceiver::new(config, macs));
        let (dev_tx, dev_rx) = channel(10);
        let (mesh_proto_tx, mesh_proto_rx) =
            tokio::sync::mpsc::unbounded_channel::<(MeshProto, MacAddress)>();
        Self {
            fleet_state: FleetState::new(dns, Duration::from_secs(60)),
            mqtt_receiver,
            msh_dev_event_tx: dev_tx,
            msh_dev_event_rx: dev_rx,
            mesh_proto_tx,
            mesh_proto_rx,
            cancellation_tokens: HashMap::new(),
        }
    }

    pub async fn next_event(&mut self) -> crate::Result<Event> {
        loop {
            tokio::select! {
                mqtt_message = async {
                    match self.mqtt_receiver.as_mut() {
                        Some(receiver) => receiver.next_message().await,
                        None => std::future::pending().await
                    }
                } => {
                    if let Some(message) = self.fleet_state.process_message(mqtt_message?)? {
                        return Ok(message);
                    }
                }
                mesh_recv = self.mesh_proto_rx.recv() => {
                    match mesh_recv {
                        Some((mesh_proto, mac_address)) => {
                            if let Some(message) = self.process_mesh_proto(mesh_proto, mac_address).await? {
                                return Ok(message);
                            }
                        }
                        None => {
                            //TODO: closed channel
                        }
                    }
                },
                msh_dev_event = self.msh_dev_event_rx.recv() => {
                    if let Some(msh_dev_event) = msh_dev_event {
                        self.process_msh_dev_event(msh_dev_event).await;
                    }
                }
                node_infos = self.fleet_state.publish_node_infos() => {
                    return Ok(Event::NodeInfos(node_infos));
                }
            }
        }
    }

    async fn process_mesh_proto(
        &mut self,
        mesh_proto: MeshProto,
        mac_address: MacAddress,
    ) -> crate::Result<Option<Event>> {
        match mesh_proto {
            MeshProto::MeshPacket(mesh_packet) => {
                self.fleet_state.process_mesh_packet(mesh_packet, mac_address)
            }
            MeshProto::Disconnected(device_node) => {
                self.process_msh_dev_event(MshDevEvent::DeviceRemoved { device_node }).await;
                Ok(None)
            }
        }
    }

    fn spawn_serial(&mut self, mut meshtastic_serial: MeshtasticSerial) -> CancellationToken {
        let mac_address = meshtastic_serial.mac_address();
        let cancellation_token = CancellationToken::new();
        let cancellation_token_clone = cancellation_token.clone();
        let mesh_proto_tx = self.mesh_proto_tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = meshtastic_serial.next_message() => {
                        mesh_proto_tx.send((msg, mac_address)).expect("Channel unexpectedly closed");
                    }
                    _ = cancellation_token.cancelled() => {
                        break;
                    }
                }
            }
        });
        cancellation_token_clone
    }

    async fn process_msh_dev_event(&mut self, msh_dev_event: MshDevEvent) {
        match msh_dev_event {
            MshDevEvent::DeviceAdded { port, device_node } => {
                //TODO: make timeout configurable
                match MeshtasticSerial::new(&port, &device_node, Duration::from_secs(12)).await {
                    Ok(msh_serial) => {
                        let mac_address = msh_serial.mac_address();
                        info!("Connected to meshtastic device: {mac_address} at {port}");
                        let token = self.spawn_serial(msh_serial);
                        self.cancellation_tokens.insert(device_node.to_owned(), token);
                    }
                    Err(err) => {
                        error!("Error connecting to {port}: {err}");
                    }
                }
            }
            MshDevEvent::DeviceRemoved { device_node } => {
                if let Entry::Occupied(occupied_entry) = self.cancellation_tokens.entry(device_node)
                {
                    // TODO: make this more informative, e.g. print device MAC address
                    warn!("Removed meshtastic device: {}", occupied_entry.key());
                    occupied_entry.get().cancel();
                    occupied_entry.remove();
                }
            }
        }
    }

    pub fn meshtastic_device_notifier(&self) -> MshDevNotifier {
        MshDevNotifier {
            dev_event_tx: self.msh_dev_event_tx.clone(),
        }
    }
}
