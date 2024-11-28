use std::fmt;

use crate::logs::HostInfo;
use chrono::{prelude::*, Duration};
use common::punch::SiPunch as CommonSiPunch;
use pyo3::prelude::*;

#[derive(Debug, Clone, PartialEq)]
#[pyclass]
pub struct SiPunch {
    #[pyo3(get)]
    pub card: u32,
    #[pyo3(get)]
    pub code: u16,
    #[pyo3(get)]
    pub time: DateTime<FixedOffset>,
    #[pyo3(get)]
    mode: u8,
    #[pyo3(get)]
    raw: [u8; 20],
}

#[derive(Debug, Clone, PartialEq)]
#[pyclass]
pub struct SiPunchLog {
    #[pyo3(get)]
    pub punch: SiPunch,
    pub latency: Duration,
    #[pyo3(get)]
    pub host_info: HostInfo,
}

#[pymethods]
impl SiPunch {
    #[staticmethod]
    pub fn new(card: u32, code: u16, time: DateTime<FixedOffset>, mode: u8) -> Self {
        Self {
            card,
            code,
            time,
            mode,
            raw: CommonSiPunch::punch_to_bytes(card, code, time.naive_local(), mode),
        }
    }

    #[staticmethod]
    pub fn from_raw(bytes: [u8; 20]) -> Self {
        let punch = CommonSiPunch::from_raw(bytes, Local::now().date_naive());

        Self {
            card: punch.card,
            code: punch.code,
            time: punch.time.and_local_timezone(Local).unwrap().fixed_offset(),
            mode: punch.mode,
            raw: bytes,
        }
    }
}

impl SiPunch {
    pub fn punches_from_payload(payload: &[u8]) -> Vec<Result<Self, std::io::Error>> {
        payload
            .chunks(20)
            .map(|chunk| {
                let partial_payload: [u8; 20] = chunk.try_into().map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("Wrong length of chunk={}", chunk.len()),
                    )
                })?;
                Ok(Self::from_raw(partial_payload))
            })
            .collect()
    }
}

#[pymethods]
impl SiPunchLog {
    #[staticmethod]
    pub fn from_raw(payload: [u8; 20], host_info: &HostInfo, now: DateTime<FixedOffset>) -> Self {
        let punch = SiPunch::from_raw(payload);
        Self {
            latency: now - punch.time,
            punch,
            host_info: host_info.clone(),
        }
    }

    #[staticmethod]
    pub fn new(punch: SiPunch, host_info: &HostInfo, now: DateTime<FixedOffset>) -> Self {
        Self {
            latency: now - punch.time,
            punch,
            host_info: host_info.clone(),
        }
    }

    pub fn __repr__(&self) -> String {
        format!("{}", self)
    }
}

impl fmt::Display for SiPunchLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} punched {} ",
            self.host_info.name, self.punch.card, self.punch.code
        )?;
        write!(f, "at {}", self.punch.time.format("%H:%M:%S.%3f"))?;
        let millis = self.latency.num_milliseconds() as f64 / 1000.0;
        write!(f, ", latency {:4.2}s", millis)
    }
}

#[cfg(test)]
mod test_punch {
    use chrono::{prelude::*, Duration};
    use common::punch::SiPunch as CommonSiPunch;

    use crate::{
        logs::HostInfo,
        punch::{SiPunch, SiPunchLog},
    };

    #[test]
    fn test_punches_from_payload() {
        let date = CommonSiPunch::last_dow(4, Local::now().date_naive());
        let time = NaiveTime::from_hms_nano_opt(10, 0, 3, 792968750).expect("Wrong time");
        let datetime =
            NaiveDateTime::new(date, time).and_local_timezone(Local).unwrap().fixed_offset();

        let punch = SiPunch::new(1715004, 47, datetime, 2);
        let payload =
            b"\xff\x02\xd3\x0d\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\x50\xe3\x03\xff\x02";

        let punches = SiPunch::punches_from_payload(payload);
        assert_eq!(punches.len(), 2);
        assert_eq!(*punches[0].as_ref().unwrap(), punch);
        assert_eq!(
            format!("{}", *punches[1].as_ref().unwrap_err()),
            "Wrong length of chunk=2"
        );
    }

    #[test]
    fn test_display() {
        let time = DateTime::parse_from_rfc3339("2023-11-23T10:00:03.793+01:00").unwrap();
        let host_info = HostInfo {
            name: "ROC1".to_owned(),
            mac_address: "abcdef123456".to_owned(),
        };
        let punch = SiPunchLog::new(
            SiPunch::new(46283, 47, time, 1),
            &host_info,
            time + Duration::milliseconds(2831),
        );

        assert_eq!(
            format!("{punch}"),
            "ROC1 46283 punched 47 at 10:00:03.793, latency 2.83s"
        );
    }
}
