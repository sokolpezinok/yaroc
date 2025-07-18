use chrono::prelude::*;
use chrono::{DateTime, Duration};
use meshtastic::Message as MeshtaticMessage;
use meshtastic::protobufs::mesh_packet::PayloadVariant;
use meshtastic::protobufs::{Data, ServiceEnvelope, Telemetry, telemetry};
use meshtastic::protobufs::{MeshPacket, PortNum, Position as PositionProto};
use std::collections::HashMap;
use std::fmt;

use crate::error::Error;
use crate::system_info::{HostInfo, MacAddress};
use yaroc_common::status::Position;

#[derive(Clone, Debug, PartialEq)]
pub struct RssiSnr {
    pub rssi_dbm: i16,
    pub snr: f32,
    pub distance: Option<(f32, String)>,
}

impl RssiSnr {
    pub fn new(rssi_dbm: i32, snr: f32) -> Option<RssiSnr> {
        match rssi_dbm {
            0 => None,
            rx_rssi => Some(RssiSnr {
                rssi_dbm: rx_rssi as i16,
                snr,
                distance: None,
            }),
        }
    }

    pub fn add_distance(&mut self, dist_m: f32, name: &str) {
        self.distance = Some((dist_m, name.to_owned()));
    }
}

pub struct PositionName {
    pub position: Position,
    pub name: String,
}

impl PositionName {
    pub fn new(position: &Position, name: &str) -> Self {
        Self {
            position: position.clone(),
            name: name.to_owned(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum MshMetrics {
    Position(Position),
    Battery { voltage: f32, percent: u32 },
    EnvironmentMetrics(f32, f32),
}

#[derive(Debug)]
pub struct MeshtasticLog {
    pub metrics: MshMetrics,
    pub host_info: HostInfo,
    pub rssi_snr: Option<RssiSnr>,
    pub timestamp: DateTime<FixedOffset>,
    latency: Duration,
}

impl MeshtasticLog {
    /// Parse MeshtasticLog from a serialized ServiceEnvelope proto.
    ///
    /// # Arguments
    /// * `payload` - The serialized ServiceEnvelope proto.
    /// * `now` - The timestamp when this proto was received.
    /// * `dns` - DNS records, mapping MAC addresses to strings
    /// * `recv_position` - Position of the node which received the proto last.
    pub fn from_service_envelope(
        payload: &[u8],
        now: DateTime<FixedOffset>,
        dns: &HashMap<MacAddress, String>,
        recv_position: Option<PositionName>,
    ) -> crate::Result<Option<Self>> {
        let service_envelope = ServiceEnvelope::decode(payload)?;
        match service_envelope.packet {
            Some(packet) => Self::from_mesh_packet(packet, now, dns, recv_position),
            None => Ok(None),
        }
    }

    pub fn from_mesh_packet(
        packet: MeshPacket,
        now: DateTime<FixedOffset>,
        dns: &HashMap<MacAddress, String>,
        recv_position: Option<PositionName>,
    ) -> crate::Result<Option<Self>> {
        match packet {
            MeshPacket {
                payload_variant: Some(PayloadVariant::Decoded(data)),
                from,
                rx_rssi,
                rx_snr,
                ..
            } => {
                let mac_address = MacAddress::Meshtastic(from);
                let name = dns.get(&mac_address).map(String::as_str).unwrap_or("Unknown");
                Self::parse_data(
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
            MeshPacket {
                payload_variant: Some(PayloadVariant::Encrypted(_)),
                ..
            } => Err(Error::EncryptionError),
            _ => Ok(None),
        }
    }

    /// Get portnum inside MeshPacket, if it exists and can be decoded.
    pub fn get_mesh_packet_portnum(mesh_packet: &MeshPacket) -> Option<i32> {
        match mesh_packet {
            MeshPacket {
                payload_variant: Some(PayloadVariant::Decoded(Data { portnum, .. })),
                ..
            } => Some(*portnum),
            _ => None,
        }
    }

    fn datetime_from_secs(timestamp: i64, tz: &impl TimeZone) -> DateTime<FixedOffset> {
        tz.timestamp_opt(timestamp, 0).unwrap().fixed_offset()
    }

    pub fn timestamp(posix_time: u32) -> DateTime<FixedOffset> {
        Self::datetime_from_secs(i64::from(posix_time), &Local)
    }

    fn parse_telemetry(
        telemetry: Telemetry,
        host_info: HostInfo,
        rssi_snr: Option<RssiSnr>,
        now: DateTime<FixedOffset>,
    ) -> Option<Self> {
        let timestamp = Self::timestamp(telemetry.time);
        match telemetry.variant {
            Some(telemetry::Variant::DeviceMetrics(metrics)) => Some(Self {
                metrics: MshMetrics::Battery {
                    voltage: metrics.voltage?,
                    percent: metrics.battery_level?,
                },
                host_info,
                rssi_snr,
                timestamp,
                latency: now - timestamp,
            }),
            Some(telemetry::Variant::EnvironmentMetrics(metrics)) => Some(Self {
                metrics: MshMetrics::EnvironmentMetrics(
                    metrics.temperature?,
                    metrics.relative_humidity?,
                ),
                host_info,
                rssi_snr,
                timestamp,
                latency: now - timestamp,
            }),
            _ => None,
        }
    }

    fn parse_position(
        position: PositionProto,
        host_info: HostInfo,
        mut rssi_snr: Option<RssiSnr>,
        now: DateTime<FixedOffset>,
        recv_position: Option<PositionName>,
    ) -> Option<Self> {
        let timestamp = Self::timestamp(position.time);
        let position = Position {
            lat: position.latitude_i? as f32 / 10_000_000.,
            lon: position.longitude_i? as f32 / 10_000_000.,
            elevation: position.altitude?,
            timestamp: Self::timestamp(position.time),
        };
        let distance = recv_position.as_ref().map(|other| position.distance_m(&other.position));
        if let Some(Ok(distance)) = distance {
            if let Some(rssi_snr) = rssi_snr.as_mut() {
                rssi_snr.add_distance(
                    distance as f32,
                    &recv_position.map_or(String::new(), |x| x.name),
                );
            }
        }

        Some(Self {
            metrics: MshMetrics::Position(position),
            host_info,
            timestamp,
            latency: now - timestamp,
            rssi_snr,
        })
    }

    fn parse_data(
        data: Data,
        host_info: HostInfo,
        now: DateTime<FixedOffset>,
        rssi_snr: Option<RssiSnr>,
        recv_position: Option<PositionName>,
    ) -> crate::Result<Option<Self>> {
        match data.portnum {
            TELEMETRY_APP => {
                let telemetry = Telemetry::decode(data.payload.as_slice())?;
                Ok(Self::parse_telemetry(telemetry, host_info, rssi_snr, now))
            }
            POSITION_APP => {
                let position = PositionProto::decode(data.payload.as_slice())?;
                Ok(Self::parse_position(
                    position,
                    host_info,
                    rssi_snr,
                    now,
                    recv_position,
                ))
            }
            _ => Ok(None),
        }
    }
}

pub(crate) const TELEMETRY_APP: i32 = PortNum::TelemetryApp as i32;
pub(crate) const POSITION_APP: i32 = PortNum::PositionApp as i32;
pub(crate) const SERIAL_APP: i32 = PortNum::SerialApp as i32;

impl fmt::Display for MeshtasticLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let timestamp = self.timestamp.format("%H:%M:%S");
        write!(f, "{} {timestamp}:", self.host_info.name)?;
        match self.metrics {
            MshMetrics::Battery { voltage, percent } => {
                write!(f, " batt {:.3}V {}%", voltage, percent)?;
            }
            MshMetrics::EnvironmentMetrics(temperature, relative_humidity) => {
                write!(f, " {temperature}°C {relative_humidity}% humid.")?;
            }
            MshMetrics::Position(Position {
                lat,
                lon,
                elevation,
                ..
            }) => {
                write!(f, " coords {:.5} {:.5} {}m", lat, lon, elevation)?;
            }
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
    use meshtastic::protobufs::{DeviceMetrics, EnvironmentMetrics, telemetry::Variant};
    use std::format;
    use std::vec::Vec;

    fn telemetry_service_envelope(
        from: u32,
        telemetry_variant: telemetry::Variant,
        timetamp_ms: u32,
        rx_rssi: i32,
        rx_snr: f32,
    ) -> Vec<u8> {
        let telemetry = Telemetry {
            time: timetamp_ms,
            variant: Some(telemetry_variant),
        };
        let data = Data {
            portnum: PortNum::TelemetryApp as i32,
            payload: telemetry.encode_to_vec(),
            ..Default::default()
        };
        let envelope = ServiceEnvelope {
            packet: Some(MeshPacket {
                from,
                payload_variant: Some(PayloadVariant::Decoded(data.clone())),
                rx_rssi,
                rx_snr,
                ..Default::default()
            }),
            ..Default::default()
        };
        envelope.encode_to_vec()
    }

    #[test]
    fn test_volt_batt() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-29T21:34:49+01:00").unwrap();
        let log_message = MeshtasticLog {
            host_info: HostInfo {
                name: "spr01".into(),
                mac_address: MacAddress::Meshtastic(0x1234),
            },
            timestamp,
            latency: Duration::milliseconds(1230),
            metrics: MshMetrics::Battery {
                voltage: 4.012,
                percent: 82,
            },
            rssi_snr: None,
        };
        assert_eq!(
            format!("{log_message}"),
            "spr01 21:34:49: batt 4.012V 82%, latency 1.23s"
        );
    }

    #[test]
    fn test_position_format() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-29T13:15:25+01:00").unwrap();
        let log_message = MeshtasticLog {
            host_info: HostInfo {
                name: "spr01".into(),
                mac_address: MacAddress::Meshtastic(0x1234),
            },
            timestamp,
            latency: Duration::milliseconds(1230),
            metrics: MshMetrics::Position(Position {
                lat: 48.29633,
                lon: 17.26675,
                elevation: 170,
                timestamp,
            }),
            rssi_snr: None,
        };
        assert_eq!(
            format!("{log_message}"),
            "spr01 13:15:25: coords 48.29633 17.26675 170m, latency 1.23s",
        );
    }

    #[test]
    fn test_position_dbm_format() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-29T13:15:25+01:00").unwrap();
        let log_message = MeshtasticLog {
            host_info: HostInfo {
                name: "spr01".into(),
                mac_address: MacAddress::Meshtastic(0x1234),
            },
            timestamp,
            latency: Duration::milliseconds(1230),
            metrics: MshMetrics::Position(Position {
                lat: 48.29633,
                lon: 17.26675,
                elevation: 170,
                timestamp,
            }),
            rssi_snr: Some(RssiSnr {
                rssi_dbm: -80,
                snr: 4.25,
                distance: Some((813., "spr02".into())),
            }),
        };
        assert_eq!(
            format!("{log_message}"),
            "spr01 13:15:25: coords 48.29633 17.26675 170m, latency 1.23s, -80dBm 4.25SNR, 0.81km \
            from spr02"
        );
    }

    #[test]
    fn test_device_metrics_parsing() {
        let device_metrics = DeviceMetrics {
            voltage: Some(3.87),
            battery_level: Some(76),
            ..Default::default()
        };
        let message = telemetry_service_envelope(
            0x123456,
            Variant::DeviceMetrics(device_metrics),
            1735157442,
            -98,
            4.0,
        );
        let now = DateTime::from_timestamp(1735157447, 0).unwrap().fixed_offset();
        let dns = HashMap::from([(MacAddress::Meshtastic(0x123456), "yaroc1".to_owned())]);
        let log_message = MeshtasticLog::from_service_envelope(&message, now, &dns, None)
            .unwrap()
            .unwrap();

        assert_eq!(
            log_message.rssi_snr,
            Some(RssiSnr {
                rssi_dbm: -98,
                snr: 4.0,
                distance: None
            })
        );
        assert_eq!(log_message.host_info.name, "yaroc1");
        assert_eq!(
            log_message.host_info.mac_address,
            MacAddress::Meshtastic(0x123456)
        );
        let timestamp = DateTime::from_timestamp(1735157442, 0).unwrap();
        assert_eq!(log_message.timestamp, timestamp);
        assert_eq!(
            log_message.metrics,
            MshMetrics::Battery {
                voltage: 3.87,
                percent: 76
            }
        );
        assert_eq!(log_message.latency, Duration::seconds(5));
    }

    #[test]
    fn test_environment_metrics_parsing() {
        let environment_metrics = EnvironmentMetrics {
            temperature: Some(47.0),
            relative_humidity: Some(84.0),
            ..Default::default()
        };
        let message = telemetry_service_envelope(
            0x123456,
            Variant::EnvironmentMetrics(environment_metrics),
            1735157442,
            -98,
            4.0,
        );
        let now = DateTime::from_timestamp(1735157447, 0).unwrap().fixed_offset();
        let log_message =
            MeshtasticLog::from_service_envelope(&message, now, &HashMap::new(), None)
                .unwrap()
                .unwrap();

        assert_eq!(
            log_message.rssi_snr,
            Some(RssiSnr {
                rssi_dbm: -98,
                snr: 4.0,
                distance: None
            })
        );
        assert_eq!(
            log_message.host_info.mac_address,
            MacAddress::Meshtastic(0x123456)
        );
        let timestamp = DateTime::from_timestamp(1735157442, 0).unwrap();
        assert_eq!(log_message.timestamp, timestamp);
        assert_eq!(log_message.latency, Duration::seconds(5));
        assert_eq!(
            log_message.metrics,
            MshMetrics::EnvironmentMetrics(47.0, 84.0)
        );
    }
}

#[cfg(test)]
mod test_time {
    use super::*;

    extern crate alloc;
    use alloc::string::ToString;

    #[test]
    fn test_timestamp() {
        let tz = FixedOffset::east_opt(3600).unwrap();
        let timestamp = MeshtasticLog::datetime_from_secs(1706523131, &tz)
            .format("%H:%M:%S.%3f")
            .to_string();
        assert_eq!("11:12:11.000", timestamp);
    }

    #[test]
    fn test_proto_timestamp_now() {
        let tz = FixedOffset::east_opt(3600).unwrap();
        let now = Local::now().with_timezone(&tz);
        let now_through_proto = MeshtasticLog::datetime_from_secs(now.timestamp(), &tz);
        let now_formatted = now.format("%Y-%m-%d %H:%M:%S").to_string();
        assert_eq!(
            now_formatted,
            now_through_proto.format("%Y-%m-%d %H:%M:%S").to_string(),
        );
    }
}
