use std::time::Duration;

use log::{error, info, warn};
use tokio::sync::mpsc::{Receiver, Sender, channel};

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
            fleet_state: FleetState::new(dns, Duration::from_secs(60)),
            mqtt_receiver,
            meshtastic_serial: None,
            meshtastic_mac: None,
            msh_dev_event_tx: tx,
            msh_dev_event_rx: rx,
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
                mesh_proto = async {
                    match self.meshtastic_serial.as_mut() {
                        Some(meshtastic_serial) => meshtastic_serial.next_message().await,
                        None => std::future::pending().await
                    }
                } => {
                    if let Some(message) = self.process_mesh_proto(mesh_proto).await? {
                        return Ok(message);
                    }
                }
                msh_dev_event = self.msh_dev_event_rx.recv() => {
                    self.process_msh_dev_event(msh_dev_event).await;
                }
                node_infos = self.fleet_state.publish_node_infos() => {
                    return Ok(Event::NodeInfos(node_infos));
                }
            }
        }
    }

    async fn process_mesh_proto(&mut self, mesh_proto: MeshProto) -> crate::Result<Option<Event>> {
        match mesh_proto {
            MeshProto::MeshPacket(mesh_packet) => {
                self.fleet_state.process_mesh_packet(mesh_packet, self.meshtastic_mac)
            }
            MeshProto::MyNodeInfo(node_info) => {
                let mac_address = MacAddress::Meshtastic(node_info.my_node_num);
                self.meshtastic_mac = Some(mac_address);
                Ok(None)
            }
            MeshProto::Disconnected => {
                self.disconnect_meshtastic(true).await;
                Ok(None)
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
                        info!("Connected to meshtastic device: {port} at {device_node}");
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
                    self.disconnect_meshtastic(false).await;
                    info!("Removed device: {device_node}");
                }
            }
            _ => {}
        }
    }

    async fn disconnect_meshtastic(&mut self, expect_err: bool) {
        if let Some(meshtastic_serial) = self.meshtastic_serial.take() {
            let res = meshtastic_serial.disconnect().await;
            if expect_err && res.is_err() {
                warn!("Disconnected meshtastic device");
            }
        }
    }

    pub fn meshtastic_device_notifier(&self) -> MshDevNotifier {
        MshDevNotifier {
            dev_event_tx: self.msh_dev_event_tx.clone(),
        }
    }
}
