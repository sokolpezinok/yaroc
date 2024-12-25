use chrono::prelude::*;
use chrono::{DateTime, Duration};
use meshtastic::protobufs::mesh_packet::PayloadVariant;
use meshtastic::protobufs::{telemetry, Data, ServiceEnvelope, Telemetry};
use meshtastic::protobufs::{MeshPacket, PortNum, Position as PositionProto};
use meshtastic::Message as MeshtaticMessage;
use std::collections::HashMap;
use std::fmt;

use crate::logs::{HostInfo, PositionName, RssiSnr};
use crate::status::Position;

#[derive(Default)]
pub struct MshLogMessage {
    pub host_info: HostInfo,
    pub voltage_battery: Option<(f32, u32)>,
    pub position: Option<Position>,
    pub rssi_snr: Option<RssiSnr>,
    pub timestamp: DateTime<FixedOffset>,
    latency: Duration,
}

impl MshLogMessage {
    pub fn timestamp(posix_time: u32) -> DateTime<FixedOffset> {
        yaroc_common::time::datetime_from_millis(i64::from(posix_time) * 1000, &Local)
    }

    fn parse_inner(
        data: Data,
        host_info: HostInfo,
        now: DateTime<FixedOffset>,
        mut rssi_snr: Option<RssiSnr>,
        recv_position: Option<PositionName>,
    ) -> Result<Option<Self>, std::io::Error> {
        match data.portnum {
            TELEMETRY_APP => {
                let telemetry = Telemetry::decode(data.payload.as_slice())?;
                let timestamp = Self::timestamp(telemetry.time);
                match telemetry.variant {
                    Some(telemetry::Variant::DeviceMetrics(metrics)) => Ok(Some(Self {
                        host_info,
                        timestamp,
                        latency: now - timestamp,
                        voltage_battery: Some((metrics.voltage, metrics.battery_level)),
                        position: None,
                        rssi_snr,
                    })),
                    _ => Ok(None),
                }
            }
            POSITION_APP => {
                let position = PositionProto::decode(data.payload.as_slice())?;
                if position.latitude_i == 0 && position.longitude_i == 0 {
                    return Ok(None);
                }
                let timestamp = Self::timestamp(position.time);
                let position = Position {
                    lat: position.latitude_i as f32 / 10_000_000.,
                    lon: position.longitude_i as f32 / 10_000_000.,
                    elevation: position.altitude,
                    timestamp: Self::timestamp(position.time),
                };
                let distance =
                    recv_position.as_ref().map(|other| position.distance_m(&other.position));
                if let Some(Ok(distance)) = distance {
                    if let Some(rssi_snr) = rssi_snr.as_mut() {
                        rssi_snr.add_distance(
                            distance as f32,
                            recv_position.map(|x| x.name).unwrap_or_default(),
                        );
                    }
                }

                Ok(Some(Self {
                    host_info,
                    timestamp,
                    latency: now - timestamp,
                    voltage_battery: None,
                    position: Some(position),
                    rssi_snr,
                }))
            }
            _ => Ok(None),
        }
    }

    pub fn from_mesh_packet(
        payload: &[u8],
        now: DateTime<FixedOffset>,
        dns: &HashMap<String, String>,
        recv_position: Option<PositionName>,
    ) -> Result<Option<Self>, std::io::Error> {
        let service_envelope = ServiceEnvelope::decode(payload)?;
        match service_envelope.packet {
            Some(MeshPacket {
                payload_variant: Some(PayloadVariant::Decoded(data)),
                from,
                rx_rssi,
                rx_snr,
                ..
            }) => {
                let mac_address = format!("{:08x}", from);
                let name = dns.get(&mac_address).map(String::as_str).unwrap_or("Unknown");
                Self::parse_inner(
                    data,
                    HostInfo {
                        name: name.to_owned(),
                        mac_address,
                    },
                    now,
                    RssiSnr::new(rx_rssi, rx_snr),
                    recv_position,
                )
            }
            Some(MeshPacket {
                payload_variant: Some(PayloadVariant::Encrypted(_)),
                ..
            }) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Encrypted message, disable encryption in MQTT!",
            )),
            _ => Ok(None),
        }
    }
}

const TELEMETRY_APP: i32 = PortNum::TelemetryApp as i32;
const POSITION_APP: i32 = PortNum::PositionApp as i32;

impl fmt::Display for MshLogMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let timestamp = self.timestamp.format("%H:%M:%S");
        write!(f, "{} {timestamp}:", self.host_info.name)?;
        if let Some((voltage, battery)) = self.voltage_battery {
            write!(f, " batt {:.3}V {}%", voltage, battery)?;
        }
        if let Some(Position {
            lat,
            lon,
            elevation,
            ..
        }) = self.position
        {
            write!(f, " coords {:.5} {:.5} {}m", lat, lon, elevation)?;
        }
        let millis = self.latency.num_milliseconds() as f64 / 1000.0;
        write!(f, ", latency {:4.2}s", millis)?;
        if let Some(RssiSnr {
            rssi_dbm,
            snr,
            distance,
        }) = &self.rssi_snr
        {
            match distance {
                None => write!(f, ", {}dBm {:.2}SNR", rssi_dbm, snr)?,
                Some((meters, name)) => write!(
                    f,
                    ", {rssi_dbm}dBm {snr:.2}SNR, {:.2}km from {name}",
                    meters / 1000.0,
                )?,
            }
        };
        Ok(())
    }
}

#[cfg(test)]
mod test_meshtastic {
    use super::*;
    use meshtastic::protobufs::{telemetry::Variant, DeviceMetrics};

    #[test]
    fn test_volt_batt() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-29T21:34:49+01:00").unwrap();
        let log_message = MshLogMessage {
            host_info: HostInfo {
                name: "spr01".to_owned(),
                mac_address: String::new(),
            },
            timestamp,
            latency: Duration::milliseconds(1230),
            voltage_battery: Some((4.012, 82)),
            ..Default::default()
        };
        assert_eq!(
            format!("{log_message}"),
            "spr01 21:34:49: batt 4.012V 82%, latency 1.23s"
        );
    }

    #[test]
    fn test_position_format() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-29T13:15:25+01:00").unwrap();
        let log_message = MshLogMessage {
            host_info: HostInfo {
                name: "spr01".to_owned(),
                mac_address: String::new(),
            },
            timestamp,
            latency: Duration::milliseconds(1230),
            position: Some(Position {
                lat: 48.29633,
                lon: 17.26675,
                elevation: 170,
                timestamp,
            }),
            ..Default::default()
        };
        assert_eq!(
            format!("{log_message}"),
            "spr01 13:15:25: coords 48.29633 17.26675 170m, latency 1.23s",
        );
    }

    #[test]
    fn test_position_dbm_format() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-29T13:15:25+01:00").unwrap();
        let log_message = MshLogMessage {
            host_info: HostInfo {
                name: "spr01".to_owned(),
                mac_address: String::new(),
            },
            timestamp,
            latency: Duration::milliseconds(1230),
            position: Some(Position {
                lat: 48.29633,
                lon: 17.26675,
                elevation: 170,
                timestamp,
            }),
            voltage_battery: None,
            rssi_snr: Some(RssiSnr {
                rssi_dbm: -80,
                snr: 4.25,
                distance: Some((813., "spr02".to_string())),
            }),
        };
        assert_eq!(
            format!("{log_message}"),
            "spr01 13:15:25: coords 48.29633 17.26675 170m, latency 1.23s, -80dBm 4.25SNR, 0.81km \
            from spr02"
        );
    }

    #[test]
    fn test_proto_parsing() {
        let device_metrics = DeviceMetrics {
            voltage: 3.87,
            battery_level: 76,
            ..Default::default()
        };
        let telemetry = Telemetry {
            time: 1735157442,
            variant: Some(Variant::DeviceMetrics(device_metrics)),
        };
        let data = Data {
            portnum: PortNum::TelemetryApp as i32,
            payload: telemetry.encode_to_vec(),
            ..Default::default()
        };
        let envelope = ServiceEnvelope {
            packet: Some(MeshPacket {
                from: 0x123456,
                payload_variant: Some(PayloadVariant::Decoded(data.clone())),
                rx_rssi: -98,
                rx_snr: 4.0,
                ..Default::default()
            }),
            ..Default::default()
        };
        let message = envelope.encode_to_vec();
        let now = DateTime::from_timestamp(1735157447, 0).unwrap().fixed_offset();
        let dns = HashMap::from([("00123456".to_owned(), "yaroc1".to_owned())]);
        let log_message =
            MshLogMessage::from_mesh_packet(&message, now, &dns, None).unwrap().unwrap();

        assert_eq!(
            log_message.rssi_snr,
            Some(RssiSnr {
                rssi_dbm: -98,
                snr: 4.0,
                distance: None
            })
        );
        assert_eq!(log_message.host_info.name, "yaroc1");
        assert_eq!(log_message.host_info.mac_address, "00123456");
        let timestamp = DateTime::from_timestamp(1735157442, 0).unwrap();
        assert_eq!(log_message.timestamp, timestamp);
        assert_eq!(log_message.voltage_battery, Some((3.87, 76)));
        assert_eq!(log_message.latency, Duration::seconds(5));
    }
}
