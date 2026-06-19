use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use yaroc_common::punch::SiPunch;
use yaroc_common::status::SignalStrength;
use yaroc_receiver::message_handler::MessageHandlerBuilder;
use yaroc_receiver::mqtt::MqttConfig as MqttConfigRs;
use yaroc_receiver::state::Event as EventRs;
use yaroc_receiver::system_info::MacAddress;

#[napi(object)]
#[derive(Serialize, Deserialize, Clone)]
pub struct DnsEntry {
    pub name: String,
    #[napi(js_name = "mac")]
    pub mac_address: String,
}

#[napi(object)]
#[derive(Serialize, Deserialize, Clone)]
pub struct MqttConfig {
    pub url: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    #[napi(js_name = "keep_alive_secs")]
    pub keep_alive_secs: u32,
    #[napi(js_name = "meshtastic_channel")]
    pub meshtastic_channel: Option<String>,
}

#[napi]
pub struct MqttClient {
    dns: Vec<DnsEntry>,
    mqtt_configs: Vec<MqttConfig>,
    cancel_token: CancellationToken,
}

#[napi]
impl MqttClient {
    #[napi(constructor)]
    pub fn new(dns: Vec<DnsEntry>, mqtt_configs: Vec<MqttConfig>) -> Self {
        MqttClient {
            dns,
            mqtt_configs,
            cancel_token: CancellationToken::new(),
        }
    }

    fn convert_event(event: EventRs) -> Option<serde_json::Value> {
        match event {
            EventRs::CellularLog(cell_log) => Some(serde_json::json!({
                "type": "CellularLog",
                "payload": {
                    "mac_address": cell_log.mac_address().to_string(),
                    "text": format!("{}", cell_log),
                }
            })),
            EventRs::SiPunches(si_punches) => Some(serde_json::json!({
                "type": "SiPunches",
                "payload": si_punches.into_iter().map(to_js_punch_log_val).collect::<Vec<_>>()
            })),
            EventRs::SiPunchesMeshtastic(si_punches, _) => Some(serde_json::json!({
                "type": "SiPunches",
                "payload": si_punches.into_iter().map(to_js_punch_log_val).collect::<Vec<_>>(),
            })),
            EventRs::SiPunch(si_punch) => Some(serde_json::json!({
                "type": "SiPunch",
                "payload": to_js_punch_val(si_punch)
            })),
            EventRs::MeshtasticLog(msh_log, service_envelope) => Some(serde_json::json!({
                "type": "MeshtasticLog",
                "payload": {
                    "text": format!("{}", msh_log),
                    "channel": service_envelope.channel_id,
                    "gateway_id": service_envelope.gateway_id,
                }
            })),
            EventRs::NodeInfos(node_infos) => Some(serde_json::json!({
                "type": "NodeInfos",
                "payload": node_infos.into_iter().map(to_js_node_info_val).collect::<Vec<_>>()
            })),
            EventRs::DeviceEvent { .. } => None,
        }
    }

    #[napi]
    pub fn start(
        &mut self,
        #[napi(ts_arg_type = "(err: Error | null, event: any) => void")] callback: JsFunction,
    ) -> Result<()> {
        let tsfn: ThreadsafeFunction<serde_json::Value, ErrorStrategy::CalleeHandled> = callback
            .create_threadsafe_function(0, |ctx| {
                let js_val = ctx.env.to_js_value(&ctx.value)?;
                Ok(vec![js_val])
            })?;

        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Debug)
            .filter_module("rumqttc::state", log::LevelFilter::Info)
            .format_timestamp_millis()
            .try_init();

        let dns = self.dns.clone();
        let mqtt_configs = self.mqtt_configs.clone();
        let cancel_token = self.cancel_token.clone();

        napi::tokio::spawn(async move {
            let dns = dns
                .into_iter()
                .map(|entry| {
                    let mac_address =
                        MacAddress::try_from(entry.mac_address.as_str()).expect("Wrong MAC format");
                    (entry.name, mac_address)
                })
                .collect::<Vec<_>>();

            let mqtt_configs = mqtt_configs
                .into_iter()
                .map(|config| MqttConfigRs {
                    url: config.url,
                    port: config.port,
                    credentials: match (config.username, config.password) {
                        (Some(u), Some(p)) => Some((u, p)),
                        _ => None,
                    },
                    keep_alive: Duration::from_secs(config.keep_alive_secs as u64),
                    meshtastic_channel: config.meshtastic_channel,
                })
                .collect::<Vec<_>>();

            let mut handler = MessageHandlerBuilder::new()
                .with_dns(dns)
                .with_mqtt_configs(mqtt_configs)
                .build();

            let init_status = serde_json::json!({
                "status": "initialized"
            });
            tsfn.call(Ok(init_status), ThreadsafeFunctionCallMode::Blocking);

            cancel_token
                .run_until_cancelled(async move {
                    loop {
                        match handler.next_event().await {
                            Ok(event) => {
                                let js_event = Self::convert_event(event);
                                if let Some(js_event) = js_event {
                                    // TODO: emit Vec<SiPunchLog> as multiple events
                                    tsfn.call(Ok(js_event), ThreadsafeFunctionCallMode::Blocking);
                                }
                            }
                            Err(err) => {
                                log::error!("{}", err);
                                let napi_err = napi::Error::from_reason(format!("{}", err));
                                tsfn.call(Err(napi_err), ThreadsafeFunctionCallMode::Blocking);
                            }
                        }
                    }
                })
                .await;
        });

        Ok(())
    }

    #[napi]
    pub fn stop(&mut self) {
        self.cancel_token.cancel();
    }
}

fn to_js_punch_val(punch: SiPunch) -> serde_json::Value {
    serde_json::json!({
        "card": punch.card,
        "code": punch.code,
        "time": punch.time.to_rfc3339(),
        "mode": punch.mode,
    })
}

fn to_js_punch_log_val(log: yaroc_receiver::logs::SiPunchLog) -> serde_json::Value {
    let rssi_dbm = log.rssi_snr.as_ref().map(|r| r.rssi_dbm as i32);
    let snr = log.rssi_snr.as_ref().map(|r| r.snr as f64);
    let hop_count = log.rssi_snr.as_ref().map(|r| r.hop_count as u32);

    serde_json::json!({
        "punch": to_js_punch_val(log.punch),
        "latency_ms": log.latency.num_milliseconds() as f64,
        "host_info": {
            "name": log.host_info.name.clone(),
            "mac_address": log.host_info.mac_address.to_string(),
        },
        "rssi_dbm": rssi_dbm,
        "snr": snr,
        "hop_count": hop_count,
    })
}

fn to_js_node_info_val(node: yaroc_receiver::state::NodeInfo) -> serde_json::Value {
    let signal_strength = match node.signal_info.signal_strength() {
        SignalStrength::Disconnected => "____",
        SignalStrength::Weak => "▂___",
        SignalStrength::Fair => "▂▄__",
        SignalStrength::Good => "▂▄▆_",
        SignalStrength::Excellent => "▂▄▆█",
    };
    serde_json::json!({
        "name": node.name,
        "signal_strength": signal_strength,
        "battery_percentage": node.battery_percentage,
        "codes": node.codes,
        "last_update": node.last_update.map(|t| t.to_rfc3339()),
        "last_punch": node.last_punch.map(|t| t.to_rfc3339()),
    })
}
