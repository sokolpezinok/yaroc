use log::error;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use yaroc_receiver::logs::CellularLogMessage;

use chrono::FixedOffset;
use yaroc_common::punch::SiPunch as SiPunchRs;
use yaroc_common::status::SignalStrength;
use yaroc_receiver::message_handler::MessageHandlerBuilder;
use yaroc_receiver::mqtt::MqttConfig as MqttConfigRs;
use yaroc_receiver::state::Event as EventRs;
use yaroc_receiver::system_info::MacAddress;

#[napi(object)]
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub name: String,
    pub mac_address: String,
}

#[napi(object)]
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SiPunch {
    pub card: u32,
    pub code: u32,
    pub time: String,
    pub mode: u32,
}

#[napi(object)]
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SiPunchLog {
    pub punch: SiPunch,
    pub latency_ms: f64,
    pub host_info: HostInfo,
    pub rssi_dbm: Option<i32>,
    pub snr: Option<f64>,
    pub hop_count: Option<u32>,
}

#[napi(object)]
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CellularLogPayload {
    pub mac_address: String,
    pub text: String,
    pub rsrp_dbm: Option<i32>,
    pub snr: Option<f64>,
    pub battery_percentage: Option<u32>,
}

#[napi(object)]
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MeshtasticLogPayload {
    pub text: String,
    pub channel: String,
    pub gateway_id: String,
}

#[napi(object)]
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    pub name: String,
    pub signal_strength: String,
    pub battery_percentage: Option<u32>,
    pub codes: Vec<u16>,
    pub last_update: Option<String>,
    pub last_punch: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", content = "payload")]
pub enum YarocEvent {
    CellularLog(CellularLogPayload),
    SiPunches(Vec<SiPunchLog>),
    SiPunch(SiPunch),
    MeshtasticLog(MeshtasticLogPayload),
    NodeInfos(Vec<NodeInfo>),
}

#[napi(object)]
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DnsEntry {
    pub name: String,
    pub mac_address: String,
}

#[napi(object)]
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MqttConfig {
    pub url: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub keep_alive_secs: u32,
    pub meshtastic_channel: Option<String>,
}

#[napi]
pub struct MqttClient {
    dns: Vec<DnsEntry>,
    mqtt_configs: Vec<MqttConfig>,
    timezone: Option<String>,
    cancel_token: CancellationToken,
}

#[napi]
impl MqttClient {
    #[napi(constructor)]
    pub fn new(
        dns: Vec<DnsEntry>,
        mqtt_configs: Vec<MqttConfig>,
        timezone: Option<String>,
    ) -> Self {
        MqttClient {
            dns,
            mqtt_configs,
            timezone,
            cancel_token: CancellationToken::new(),
        }
    }

    fn convert_event(event: EventRs) -> Option<YarocEvent> {
        match event {
            EventRs::CellularLog(cell_log) => {
                let CellularLogMessage::MCH(ref mch) = cell_log else {
                    return None;
                };
                let rsrp = mch.mini_call_home.signal_info.as_ref().map(|s| s.rsrp_dbm as i32);
                let snr = mch.mini_call_home.signal_info.as_ref().map(|s| s.snr_cb as f64 / 10.0);
                let battery = mch.mini_call_home.batt_percents.map(|b| b as u32);
                Some(YarocEvent::CellularLog(CellularLogPayload {
                    mac_address: cell_log.mac_address().to_string(),
                    text: format!("{}", cell_log),
                    rsrp_dbm: rsrp,
                    snr,
                    battery_percentage: battery,
                }))
            }
            EventRs::SiPunches(si_punches) | EventRs::SiPunchesMeshtastic(si_punches, _) => Some(
                YarocEvent::SiPunches(si_punches.into_iter().map(to_js_punch_log).collect()),
            ),
            EventRs::SiPunch(si_punch) => Some(YarocEvent::SiPunch(to_js_punch(si_punch))),
            EventRs::MeshtasticLog(msh_log, service_envelope) => {
                Some(YarocEvent::MeshtasticLog(MeshtasticLogPayload {
                    text: format!("{}", msh_log),
                    channel: service_envelope.channel_id,
                    gateway_id: service_envelope.gateway_id,
                }))
            }
            EventRs::NodeInfos(node_infos) => Some(YarocEvent::NodeInfos(
                node_infos.into_iter().map(to_js_node_info).collect(),
            )),
            EventRs::DeviceEvent { .. } => None,
        }
    }

    #[napi]
    pub fn start(
        &mut self,
        #[napi(ts_arg_type = "(err: Error | null, event: YarocEvent) => void")]
        callback: JsFunction,
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
        let timezone = self.timezone.clone();

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

            let mut builder =
                MessageHandlerBuilder::new().with_dns(dns).with_mqtt_configs(mqtt_configs);

            if let Some(ref tz_str) = timezone {
                match parse_timezone(tz_str) {
                    Ok(parsed_tz) => {
                        builder = builder.with_timezone(parsed_tz);
                    }
                    Err(err) => {
                        error!("{}", err);
                        let napi_err = napi::Error::from_reason(err);
                        tsfn.call(Err(napi_err), ThreadsafeFunctionCallMode::Blocking);
                        return;
                    }
                }
            }

            let mut handler = builder.build();
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
                                    if let Ok(js_val) = serde_json::to_value(&js_event) {
                                        tsfn.call(Ok(js_val), ThreadsafeFunctionCallMode::Blocking);
                                    }
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

fn to_js_punch(punch: SiPunchRs) -> SiPunch {
    SiPunch {
        card: punch.card,
        code: punch.code as u32,
        time: punch.time.to_rfc3339(),
        mode: punch.mode as u32,
    }
}

fn to_js_punch_log(log: yaroc_receiver::logs::SiPunchLog) -> SiPunchLog {
    let rssi_dbm = log.rssi_snr.as_ref().map(|r| r.rssi_dbm as i32);
    let snr = log.rssi_snr.as_ref().map(|r| r.snr as f64);
    let hop_count = log.rssi_snr.as_ref().map(|r| r.hop_count as u32);

    SiPunchLog {
        punch: to_js_punch(log.punch),
        latency_ms: log.latency.num_milliseconds() as f64,
        host_info: HostInfo {
            name: log.host_info.name.clone(),
            mac_address: log.host_info.mac_address.to_string(),
        },
        rssi_dbm,
        snr,
        hop_count,
    }
}

fn to_js_node_info(node: yaroc_receiver::state::NodeInfo) -> NodeInfo {
    let signal_strength = match node.signal_info.signal_strength() {
        SignalStrength::Disconnected => "____",
        SignalStrength::Weak => "▂___",
        SignalStrength::Fair => "▂▄__",
        SignalStrength::Good => "▂▄▆_",
        SignalStrength::Excellent => "▂▄▆█",
    };
    NodeInfo {
        name: node.name,
        signal_strength: signal_strength.to_string(),
        battery_percentage: node.battery_percentage.map(|b| b as u32),
        codes: node.codes,
        last_update: node.last_update.map(|t| t.to_rfc3339()),
        last_punch: node.last_punch.map(|t| t.to_rfc3339()),
    }
}

fn parse_timezone(tz: &str) -> std::result::Result<FixedOffset, String> {
    if tz == "UTC" || tz == "GMT" {
        return Ok(FixedOffset::east_opt(0).unwrap());
    }

    // Try parsing ISO 8601 offset format: +HH:MM or -HH:MM
    let chars: Vec<char> = tz.chars().collect();
    if chars.len() == 6 && (chars[0] == '+' || chars[0] == '-') && chars[3] == ':' {
        if let (Ok(hours), Ok(minutes)) = (
            chars[1..3].iter().collect::<String>().parse::<i32>(),
            chars[4..6].iter().collect::<String>().parse::<i32>(),
        ) {
            if (0..24).contains(&hours) && (0..60).contains(&minutes) {
                let offset_secs = hours * 3600 + minutes * 60;
                return if chars[0] == '+' {
                    FixedOffset::east_opt(offset_secs)
                        .ok_or_else(|| "Offset out of bounds".to_string())
                } else {
                    FixedOffset::west_opt(offset_secs)
                        .ok_or_else(|| "Offset out of bounds".to_string())
                };
            }
        }
    }

    // Try parsing as integer seconds offset (e.g. 7200 for UTC+2)
    if let Ok(offset_secs) = tz.parse::<i32>() {
        return if offset_secs >= 0 {
            FixedOffset::east_opt(offset_secs).ok_or_else(|| "Offset out of bounds".to_owned())
        } else {
            FixedOffset::west_opt(-offset_secs).ok_or_else(|| "Offset out of bounds".to_owned())
        };
    }

    Err(format!(
        "Invalid timezone offset format: '{}'. Supported formats are '+HH:MM', '-HH:MM', '+HHMM', '-HHMM', 'UTC' and 'GMT'.",
        tz
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaroc_common::status::{CellNetworkType, CellSignalInfo, MiniCallHome};
    use yaroc_receiver::logs::MiniCallHomeLog;
    use yaroc_receiver::system_info::HostInfo as HostInfoRs;

    #[test]
    fn test_parse_timezone() {
        assert_eq!(
            parse_timezone("UTC").unwrap(),
            FixedOffset::east_opt(0).unwrap()
        );
        assert_eq!(
            parse_timezone("GMT").unwrap(),
            FixedOffset::east_opt(0).unwrap()
        );
        assert_eq!(
            parse_timezone("+02:00").unwrap(),
            FixedOffset::east_opt(7200).unwrap()
        );
        assert_eq!(
            parse_timezone("-05:30").unwrap(),
            FixedOffset::west_opt(19800).unwrap()
        );
        assert_eq!(
            parse_timezone("3600").unwrap(),
            FixedOffset::east_opt(3600).unwrap()
        );
        assert_eq!(
            parse_timezone("-7200").unwrap(),
            FixedOffset::west_opt(7200).unwrap()
        );
        assert!(parse_timezone("Invalid").is_err());
        assert!(parse_timezone("+2:00").is_err());
        assert!(parse_timezone("+02:0").is_err());
    }

    #[test]
    fn test_convert_event_cellular_log() {
        let host_info = HostInfoRs {
            name: "TestNode".to_string(),
            mac_address: MacAddress::try_from("1234567890ab").unwrap(),
        };

        // Case 1: MCH CellularLog
        let mut mch = MiniCallHome::default();
        mch.signal_info = Some(CellSignalInfo {
            network_type: CellNetworkType::Lte,
            rsrp_dbm: -85,
            snr_cb: 120, // 12.0 dB
            cellid: None,
        });
        mch.batt_percents = Some(88);

        let cell_log_msg = CellularLogMessage::MCH(MiniCallHomeLog {
            mini_call_home: mch,
            host_info: host_info.clone(),
            latency: chrono::Duration::zero(),
        });

        let event = EventRs::CellularLog(cell_log_msg);
        let js_event = MqttClient::convert_event(event).unwrap();

        if let YarocEvent::CellularLog(payload) = js_event {
            assert_eq!(payload.mac_address, "1234567890ab");
            assert_eq!(payload.rsrp_dbm, Some(-85));
            assert_eq!(payload.snr, Some(12.0));
            assert_eq!(payload.battery_percentage, Some(88));
        } else {
            panic!("Expected CellularLog event");
        }

        // Case 2: Disconnected CellularLog
        let cell_log_disconnected = CellularLogMessage::Disconnected {
            host_info,
            client: "test_client".to_string(),
        };
        let event_disc = EventRs::CellularLog(cell_log_disconnected);
        let js_event_disc = MqttClient::convert_event(event_disc);
        assert!(js_event_disc.is_none());
    }
}
