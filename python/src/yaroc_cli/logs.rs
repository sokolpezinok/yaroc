//! Domain log decoding and formatting for MiniCallHome CSV export and modem AT response logs.

use std::fmt;
use std::io::Write;

use chrono::Local;
use femtopb::Message as _;
use log::{error, info, warn};
use postcard::from_bytes;
use yaroc_common::at::response::LoggedAtResponse;
use yaroc_common::proto::MiniCallHome as MiniCallHomeProto;
use yaroc_common::status::MiniCallHome;
use yaroc_common::usb::UsbResponse;

/// A single formatted CSV record derived from a [`MiniCallHome`] status log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MchCsvRow {
    pub timestamp: String,
    pub batt_mv: String,
    pub batt_percents: String,
    pub cpu_temperature: String,
    pub network_type: String,
    pub rsrp_dbm: String,
    pub snr_db: String,
    pub cellid: String,
}

impl MchCsvRow {
    /// CSV header line matching the columns produced by [`MchCsvRow`]'s [`fmt::Display`] implementation.
    pub const HEADER: &'static str =
        "timestamp,batt_mv,batt_percents,cpu_temperature,network_type,rsrp_dbm,snr_db,cellid";

    /// Converts a [`MiniCallHome`] status entry into a formatted [`MchCsvRow`],
    /// converting any timestamp to the local timezone in RFC 3339 format.
    pub fn from_mch(mch: &MiniCallHome) -> Self {
        let timestamp = mch.timestamp.map_or_default(|t| t.with_timezone(&Local).to_rfc3339());
        let batt_mv = mch.batt_mv.map_or_default(|v| v.to_string());
        let batt_percents = mch.batt_percents().map_or_default(|p| p.to_string());
        let cpu_temperature = mch.cpu_temperature.map_or_default(|t| t.to_string());

        let (network_type, rsrp_dbm, snr_db, cellid) =
            if let Some(ref signal_info) = mch.signal_info {
                (
                    format!("{:?}", signal_info.network_type),
                    signal_info.rsrp_dbm.to_string(),
                    format!("{:.1}", signal_info.snr_cb as f32 / 10.0),
                    signal_info.cellid.map_or_default(|id| format!("{id:X}")),
                )
            } else {
                (String::new(), String::new(), String::new(), String::new())
            };

        Self {
            timestamp,
            batt_mv,
            batt_percents,
            cpu_temperature,
            network_type,
            rsrp_dbm,
            snr_db,
            cellid,
        }
    }
}

impl fmt::Display for MchCsvRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{},{},{},{},{},{},{},{}",
            self.timestamp,
            self.batt_mv,
            self.batt_percents,
            self.cpu_temperature,
            self.network_type,
            self.rsrp_dbm,
            self.snr_db,
            self.cellid,
        )
    }
}

/// Decodes streamed [`UsbResponse::MiniCallHomeLog`] frames and writes them as CSV to `writer`.
pub fn write_mch_logs_to_csv<I, W>(responses: I, writer: &mut W) -> Result<(), String>
where
    I: IntoIterator<Item = Result<UsbResponse, String>>,
    W: Write,
{
    writeln!(writer, "{}", MchCsvRow::HEADER)
        .map_err(|e| format!("Failed to write CSV header: {e}"))?;

    for item in responses {
        match item? {
            UsbResponse::MiniCallHomeLog(buf) => {
                let mch = MiniCallHomeProto::decode(buf.as_slice())
                    .map_err(From::from)
                    .and_then(MiniCallHome::try_from);
                match mch {
                    Ok(mch) => {
                        let row = MchCsvRow::from_mch(&mch);
                        writeln!(writer, "{row}")
                            .map_err(|e| format!("Failed to write CSV row: {e}"))?;
                    }
                    Err(e) => {
                        error!("Failed to convert MiniCallHomeProto to MiniCallHome: {e}");
                    }
                }
            }
            r => warn!("Unexpected response: {r:?}"),
        }
    }
    writer.flush().map_err(|e| format!("Failed to flush CSV writer: {e}"))?;
    Ok(())
}

/// Deserializes a [`LoggedAtResponse`] from Postcard bytes and converts its timestamp
/// to the local timezone offset.
pub fn decode_logged_at_response(buf: &[u8]) -> Result<LoggedAtResponse, String> {
    let mut log = from_bytes::<LoggedAtResponse>(buf)
        .map_err(|e| format!("Failed to deserialize LoggedAtResponse: {e}"))?;
    log.timestamp = log.timestamp.with_timezone(&Local).fixed_offset();
    Ok(log)
}

/// Iterates over streamed [`UsbResponse::LoggedAtResponseLog`] frames and logs each decoded entry.
pub fn dump_logged_at_response_logs<I>(responses: I)
where
    I: IntoIterator<Item = Result<UsbResponse, String>>,
{
    for item in responses {
        match item {
            Ok(UsbResponse::LoggedAtResponseLog(buf)) => {
                match decode_logged_at_response(buf.as_slice()) {
                    Ok(log) => info!("{log:?}"),
                    Err(e) => error!("{e}"),
                }
            }
            Ok(r) => warn!("Unexpected response: {r:?}"),
            Err(e) => {
                error!("Error reading log: {e}");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use postcard::to_stdvec;
    use yaroc_common::at::response::AtResponse;
    use yaroc_common::proto::{CellNetworkType as ProtoCellNetworkType, Timestamp};
    use yaroc_common::status::{CellNetworkType, CellSignalInfo};

    use super::*;

    #[test]
    fn test_mch_csv_row_with_and_without_signal_info() {
        let ts = DateTime::parse_from_rfc3339("2026-07-25T10:00:00Z").unwrap();
        let expected_ts = ts.with_timezone(&Local).to_rfc3339();

        let mch_full = MiniCallHome {
            cpu_temperature: Some(24.5),
            batt_mv: Some(4200),
            timestamp: Some(ts),
            signal_info: Some(CellSignalInfo {
                network_type: CellNetworkType::LteM,
                rsrp_dbm: -95,
                snr_cb: -25,
                cellid: Some(0xABCDEF),
            }),
            ..Default::default()
        };
        let row_full = MchCsvRow::from_mch(&mch_full);
        assert_eq!(
            row_full.to_string(),
            format!("{expected_ts},4200,100,24.5,LteM,-95,-2.5,ABCDEF")
        );

        let mch_empty = MiniCallHome::default();
        let row_empty = MchCsvRow::from_mch(&mch_empty);
        assert_eq!(row_empty.to_string(), ",,,,,,,");
    }

    #[test]
    fn test_csv_serialization() {
        let mch_proto = MiniCallHomeProto {
            freq: 32,
            millivolts: 3600,
            network_type: femtopb::EnumValue::Known(ProtoCellNetworkType::LteM),
            rsrp_dbm: -100,
            signal_snr_cb: 15,
            cellid: 0x12ABCD,
            time: Some(Timestamp {
                millis_epoch: 1782512139000,
                ..Default::default()
            }),
            totaldatarx: 500,
            totaldatatx: 600,
            ..Default::default()
        };

        let mut buf = [0u8; 100];
        let mut slice = buf.as_mut_slice();
        mch_proto.encode(&mut slice).unwrap();
        let encoded_len = mch_proto.encoded_len();

        let response =
            UsbResponse::MiniCallHomeLog(heapless::Vec::from_slice(&buf[..encoded_len]).unwrap());
        let responses = vec![Ok(response)];

        let mut csv_buf = Vec::new();
        write_mch_logs_to_csv(responses, &mut csv_buf).unwrap();

        let csv_str = String::from_utf8(csv_buf).unwrap();
        let lines: Vec<&str> = csv_str.trim().lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], MchCsvRow::HEADER);
        let expected_time = DateTime::from_timestamp_millis(1782512139000)
            .unwrap()
            .with_timezone(&Local)
            .to_rfc3339();
        assert!(lines[1].contains(&expected_time));
        assert!(lines[1].contains("3600"));
        assert!(lines[1].contains("LteM"));
        assert!(lines[1].contains("-100"));
        assert!(lines[1].contains("1.5"));
        assert!(lines[1].contains("12ABCD"));
    }

    #[test]
    fn test_csv_serialization_edge_cases() {
        // Non-MCH Log variants and corrupted payloads should be skipped
        let responses = vec![
            Ok(UsbResponse::Ok),
            // Invalid protobuf payload
            Ok(UsbResponse::MiniCallHomeLog(
                heapless::Vec::from_slice(&[0xFF; 10]).unwrap(),
            )),
        ];
        let mut csv_buf = Vec::new();
        write_mch_logs_to_csv(responses, &mut csv_buf).unwrap();
        let csv_str = String::from_utf8(csv_buf).unwrap();
        assert_eq!(csv_str.trim(), MchCsvRow::HEADER);
    }

    #[test]
    fn test_decode_logged_at_response_success_and_invalid() {
        let utc_ts = DateTime::parse_from_rfc3339("2026-07-25T10:00:00Z").unwrap();
        let original = LoggedAtResponse {
            timestamp: utc_ts,
            response: AtResponse::new(heapless::Vec::new(), "+CSQ"),
        };
        let bytes = to_stdvec(&original).unwrap();

        let decoded = decode_logged_at_response(&bytes).unwrap();
        assert_eq!(
            decoded.timestamp,
            utc_ts.with_timezone(&Local).fixed_offset()
        );
        assert_eq!(decoded.response, original.response);

        let err = decode_logged_at_response(&[0xFF, 0x00]).unwrap_err();
        assert!(err.contains("Failed to deserialize LoggedAtResponse"));
    }

    #[test]
    fn test_dump_logged_at_response_logs_handles_mixed_responses() {
        let utc_ts = DateTime::parse_from_rfc3339("2026-07-25T10:00:00Z").unwrap();
        let valid_log = LoggedAtResponse {
            timestamp: utc_ts,
            response: AtResponse::new(heapless::Vec::new(), "+CSQ"),
        };
        let valid_bytes = to_stdvec(&valid_log).unwrap();

        let responses = vec![
            Ok(UsbResponse::LoggedAtResponseLog(
                heapless::Vec::from_slice(&valid_bytes).unwrap(),
            )),
            // Corrupted payload
            Ok(UsbResponse::LoggedAtResponseLog(
                heapless::Vec::from_slice(&[0xFF, 0x00]).unwrap(),
            )),
            // Unexpected response variant
            Ok(UsbResponse::Ok),
            // Stream read error stops iteration cleanly
            Err("mock read error".to_string()),
        ];

        dump_logged_at_response_logs(responses);
    }
}
