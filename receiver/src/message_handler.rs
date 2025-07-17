use log::{error, info};
use tokio::sync::mpsc::{Receiver, Sender, channel};

use crate::error::Error;
use crate::meshtastic_serial::{MeshProto, MeshtasticSerial};
use crate::mqtt::{MqttConfig, MqttReceiver};
use crate::state::{FleetState, Message};
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
    meshtastic_serial: Option<MeshtasticSerial>,
    meshtastic_mac: Option<MacAddress>,
    msh_dev_event_rx: Receiver<MshDevEvent>,
    msh_dev_event_tx: Sender<MshDevEvent>,
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
        let (tx, rx) = channel(10);
        Self {
            fleet_state: FleetState::new(dns),
            mqtt_receiver,
            meshtastic_serial: None,
            meshtastic_mac: None,
            msh_dev_event_tx: tx,
            msh_dev_event_rx: rx,
        }
    }

    pub async fn next_message(&mut self) -> crate::Result<Message> {
        loop {
            tokio::select! {
                mqtt_message = async {
                    match self.mqtt_receiver.as_mut() {
                        Some(receiver) => receiver.next_message().await,
                        None => std::future::pending().await
                    }
                } => {
                    return self.fleet_state.process_message(mqtt_message?);
                }
                mesh_proto = async {
                    match self.meshtastic_serial.as_mut() {
                        Some(meshtastic_serial) => meshtastic_serial.next_message().await,
                        None => std::future::pending().await
                    }
                } => {
                    match mesh_proto {
                        MeshProto::MeshPacket(mesh_packet) => {
                            if let Some(message) =
                                self.fleet_state.process_mesh_packet(mesh_packet, self.meshtastic_mac)?
                            {
                                return Ok(message);
                            }
                        }
                        MeshProto::MyNodeInfo(node_info) => {
                            let mac_address = MacAddress::Meshtastic(node_info.my_node_num);
                            self.meshtastic_mac = Some(mac_address);
                        }
                        MeshProto::Disconnected => todo!(),
                    }
                }
                msh_dev_event = self.msh_dev_event_rx.recv() => {
                    self.process_msh_dev_event(msh_dev_event).await;
                }
            }
        }
    }

    async fn process_msh_dev_event(&mut self, msh_dev_event: Option<MshDevEvent>) {
        match msh_dev_event {
            Some(MshDevEvent::DeviceAdded { port, device_node }) => {
                if self.meshtastic_serial.is_some() {
                    return;
                }
                match MeshtasticSerial::new(&port, &device_node).await {
                    Ok(msh_serial) => {
                        self.meshtastic_serial = Some(msh_serial);
                        info!("Connected to device: {port} at {device_node}");
                    }
                    Err(err) => {
                        error!("Error connecting to {port}: {err}");
                    }
                }
            }
            Some(MshDevEvent::DeviceRemoved { device_node }) => {
                if self
                    .meshtastic_serial
                    .as_ref()
                    .is_some_and(|msh_serial| msh_serial.device_node() == device_node)
                {
                    if let Some(meshtastic_serial) = self.meshtastic_serial.take() {
                        let _ = meshtastic_serial
                            .disconnect()
                            .await
                            .inspect_err(|e| error!("Error while disconnecting: {e}"));
                    }
                    info!("Removed device: {device_node}");
                }
            }
            _ => {}
        }
    }

    pub fn meshtastic_device_notifier(&self) -> MshDevNotifier {
        MshDevNotifier {
            dev_event_tx: self.msh_dev_event_tx.clone(),
        }
    }

    pub fn node_infos(&self) -> Vec<crate::state::NodeInfo> {
        self.fleet_state.node_infos()
    }
}
