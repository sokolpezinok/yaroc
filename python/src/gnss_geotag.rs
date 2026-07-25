use std::io::Write;
use std::path::Path;

use chrono::{DateTime, FixedOffset, Local};
use femtopb::Message as _;
use yaroc_common::proto::MiniCallHome as MiniCallHomeProto;
use yaroc_common::status::{CellSignalInfo, MiniCallHome};
use yaroc_common::usb::UsbResponse;

#[derive(Debug, Clone, PartialEq)]
pub struct GpxPoint {
    pub timestamp: DateTime<FixedOffset>,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MchRecord {
    pub timestamp: DateTime<FixedOffset>,
    pub signal_info: Option<CellSignalInfo>,
}

/// Parses GPX XML content and extracts sorted track points.
pub fn parse_gpx(gpx_content: &str) -> Result<Vec<GpxPoint>, String> {
    let doc = roxmltree::Document::parse(gpx_content)
        .map_err(|e| format!("Failed to parse GPX XML: {e}"))?;

    let mut points = Vec::new();
    for node in doc.descendants() {
        if node.is_element() && node.tag_name().name() == "trkpt" {
            let lat_str =
                node.attribute("lat").ok_or_else(|| "trkpt missing lat attribute".to_string())?;
            let lon_str =
                node.attribute("lon").ok_or_else(|| "trkpt missing lon attribute".to_string())?;

            let lat: f64 =
                lat_str.parse().map_err(|e| format!("Invalid lat value '{lat_str}': {e}"))?;
            let lon: f64 =
                lon_str.parse().map_err(|e| format!("Invalid lon value '{lon_str}': {e}"))?;

            let time_node = node
                .children()
                .find(|c| c.is_element() && c.tag_name().name() == "time")
                .ok_or_else(|| "trkpt missing time element".to_string())?;
            let time_str =
                time_node.text().ok_or_else(|| "time element has no text".to_string())?.trim();
            let timestamp = DateTime::parse_from_rfc3339(time_str)
                .map_err(|e| format!("Invalid ISO/RFC3339 timestamp '{time_str}': {e}"))?;

            points.push(GpxPoint {
                timestamp,
                lat,
                lon,
            });
        }
    }

    points.sort_by_key(|p| p.timestamp);
    Ok(points)
}

/// Extracts MiniCallHome records from UsbResponse slice.
pub fn extract_mch_records(responses: &[UsbResponse]) -> Vec<MchRecord> {
    let mut records = Vec::new();
    for response in responses {
        if let UsbResponse::MiniCallHomeLog(buf) = response {
            let mch = MiniCallHomeProto::decode(buf.as_slice())
                .map_err(From::from)
                .and_then(MiniCallHome::try_from);
            if let Ok(mch) = mch
                && let Some(timestamp) = mch.timestamp
            {
                records.push(MchRecord {
                    timestamp,
                    signal_info: mch.signal_info,
                });
            }
        }
    }
    records
}

/// Geotags MiniCallHome records with GPX track points using linear interpolation and outputs CSV.
pub fn geotag_mch_logs<W: Write>(
    gpx_points: &[GpxPoint],
    mch_records: &[MchRecord],
    writer: &mut W,
) -> Result<(), String> {
    if gpx_points.len() < 2 {
        return Err("Need at least 2 GPX track points for interpolation".to_string());
    }

    writeln!(writer, "lat,lon,time,rsrp,snr,ecl,cellid")
        .map_err(|e| format!("Failed to write CSV header: {e}"))?;

    let mut sorted_records: Vec<&MchRecord> = mch_records.iter().collect();
    sorted_records.sort_by_key(|r| r.timestamp);

    let mut ptr = 0;
    for record in sorted_records {
        let time = record.timestamp;
        while ptr + 1 < gpx_points.len() && time > gpx_points[ptr + 1].timestamp {
            ptr += 1;
        }

        if ptr + 1 >= gpx_points.len() {
            break;
        }
        if time < gpx_points[ptr].timestamp {
            continue;
        }

        let t0 = gpx_points[ptr].timestamp;
        let t1 = gpx_points[ptr + 1].timestamp;
        let delta_ms = (t1 - t0).num_milliseconds() as f64;
        let q = if delta_ms <= 0.0 {
            0.0
        } else {
            (time - t0).num_milliseconds() as f64 / delta_ms
        };

        let lat = (1.0 - q) * gpx_points[ptr].lat + q * gpx_points[ptr + 1].lat;
        let lon = (1.0 - q) * gpx_points[ptr].lon + q * gpx_points[ptr + 1].lon;

        let (rsrp, snr, ecl, cellid_str) = if let Some(ref info) = record.signal_info {
            (
                info.rsrp_dbm.to_string(),
                format!("{:.1}", info.snr_cb as f32 / 10.0),
                format!("{:?}", info.network_type),
                info.cellid.map(|id| format!("{id:X}")).unwrap_or_else(|| "N/A".to_string()),
            )
        } else {
            (
                String::new(),
                String::new(),
                String::new(),
                "N/A".to_string(),
            )
        };

        let time_str = time.with_timezone(&Local).to_rfc3339();
        writeln!(
            writer,
            "{lat},{lon},{time_str},{rsrp},{snr},{ecl},{cellid_str}"
        )
        .map_err(|e| format!("Failed to write CSV row: {e}"))?;
    }

    writer.flush().map_err(|e| format!("Failed to flush CSV writer: {e}"))?;
    Ok(())
}

pub fn geotag_mch_responses<P: AsRef<Path>, W: Write>(
    gpx_path: P,
    responses: &[UsbResponse],
    writer: &mut W,
) -> Result<(), String> {
    let gpx_content = std::fs::read_to_string(gpx_path.as_ref()).map_err(|e| {
        format!(
            "Failed to read GPX file {}: {e}",
            gpx_path.as_ref().display()
        )
    })?;
    let gpx_points = parse_gpx(&gpx_content)?;
    let mch_records = extract_mch_records(responses);
    geotag_mch_logs(&gpx_points, &mch_records, writer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaroc_common::status::CellNetworkType;

    #[test]
    fn test_parse_gpx() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="test" xmlns="http://www.topografix.com/GPX/1/1">
  <trk>
    <name>Test Track</name>
    <trkseg>
      <trkpt lat="49.1000" lon="16.5000">
        <time>2026-07-25T10:00:00Z</time>
      </trkpt>
      <trkpt lat="49.2000" lon="16.6000">
        <time>2026-07-25T10:10:00Z</time>
      </trkpt>
    </trkseg>
  </trk>
</gpx>"#;

        let points = parse_gpx(xml).unwrap();
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].lat, 49.1000);
        assert_eq!(points[0].lon, 16.5000);
        assert_eq!(points[1].lat, 49.2000);
        assert_eq!(points[1].lon, 16.6000);
    }

    #[test]
    fn test_geotag_interpolation() {
        let gpx_points = vec![
            GpxPoint {
                timestamp: DateTime::parse_from_rfc3339("2026-07-25T10:00:00Z").unwrap(),
                lat: 40.0,
                lon: 10.0,
            },
            GpxPoint {
                timestamp: DateTime::parse_from_rfc3339("2026-07-25T10:10:00Z").unwrap(),
                lat: 50.0,
                lon: 20.0,
            },
        ];

        let mch_records = vec![
            MchRecord {
                timestamp: DateTime::parse_from_rfc3339("2026-07-25T10:05:00Z").unwrap(),
                signal_info: Some(CellSignalInfo {
                    network_type: CellNetworkType::LteM,
                    rsrp_dbm: -90,
                    snr_cb: 20,
                    cellid: Some(0x12ABCD),
                }),
            },
            MchRecord {
                timestamp: DateTime::parse_from_rfc3339("2026-07-25T10:08:00Z").unwrap(),
                signal_info: None,
            },
        ];

        let mut buf = Vec::new();
        geotag_mch_logs(&gpx_points, &mch_records, &mut buf).unwrap();
        let csv = String::from_utf8(buf).unwrap();

        let lines: Vec<&str> = csv.trim().split('\n').collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "lat,lon,time,rsrp,snr,ecl,cellid");
        let expected_time1 = DateTime::parse_from_rfc3339("2026-07-25T10:05:00Z")
            .unwrap()
            .with_timezone(&Local)
            .to_rfc3339();
        let expected_time2 = DateTime::parse_from_rfc3339("2026-07-25T10:08:00Z")
            .unwrap()
            .with_timezone(&Local)
            .to_rfc3339();
        assert_eq!(
            lines[1],
            format!("45,15,{expected_time1},-90,2.0,LteM,12ABCD")
        );
        assert_eq!(lines[2], format!("48,18,{expected_time2},,,,N/A"));
    }
}
