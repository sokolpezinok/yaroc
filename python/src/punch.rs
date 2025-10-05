use std::fmt;

use chrono::{Duration, prelude::*};
use pyo3::prelude::*;
use yaroc_common::punch::{RawPunch, SiPunch as SiPunchRs};
use yaroc_receiver::logs::SiPunchLog as SiPunchLogRs;

use crate::status::HostInfo;

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
    raw: RawPunch,
}

impl From<SiPunchRs> for SiPunch {
    fn from(punch: SiPunchRs) -> Self {
        Self {
            card: punch.card,
            code: punch.code,
            time: punch.time,
            mode: punch.mode,
            raw: punch.raw,
        }
    }
}

#[pymethods]
impl SiPunch {
    #[staticmethod]
    pub fn new(card: u32, code: u16, time: DateTime<FixedOffset>, mode: u8) -> Self {
        SiPunchRs::new(card, code, time, mode).into()
    }

    #[staticmethod]
    pub fn from_raw(raw: &[u8], now: DateTime<FixedOffset>) -> Option<Self> {
        let (punch, _rest) = SiPunchRs::from_bytes(raw, now.date_naive(), now.offset())?;
        Some(punch.into())
    }
}

#[pyclass]
#[derive(Clone)]
pub struct SiPunchLog {
    #[pyo3(get)]
    pub punch: SiPunch,
    #[pyo3(get)]
    pub latency: Duration,
    #[pyo3(get)]
    pub host_info: HostInfo,
}

impl From<SiPunchLogRs> for SiPunchLog {
    fn from(punch_log: SiPunchLogRs) -> Self {
        Self {
            punch: punch_log.punch.into(),
            latency: punch_log.latency,
            host_info: punch_log.host_info.into(),
        }
    }
}

#[pymethods]
impl SiPunchLog {
    #[staticmethod]
    pub fn new(punch: SiPunch, host_info: &HostInfo, now: DateTime<FixedOffset>) -> Self {
        Self {
            latency: now - punch.time,
            punch,
            host_info: host_info.clone(),
        }
    }

    pub fn is_meshtastic(&self) -> bool {
        self.host_info.mac_address().is_meshtastic()
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
            self.host_info.name(),
            self.punch.card,
            self.punch.code
        )?;
        write!(f, "at {}", self.punch.time.format("%H:%M:%S.%3f"))?;
        let millis = self.latency.num_milliseconds() as f64 / 1000.0;
        write!(f, ", latency {:4.2}s", millis)
    }
}

#[cfg(test)]
mod test_punch {
    use super::*;
    use yaroc_receiver::system_info::{HostInfo, MacAddress};

    #[test]
    fn test_display() {
        let time = DateTime::parse_from_rfc3339("2023-11-23T10:00:03.793+01:00").unwrap();
        let host_info = HostInfo::new("ROC1", MacAddress::Full(0x123456789012));
        let log = SiPunchLog::new(
            SiPunch::new(46283, 47, time, 1),
            &host_info.into(),
            time + Duration::milliseconds(2831),
        );

        assert_eq!(
            format!("{log}"),
            "ROC1 46283 punched 47 at 10:00:03.793, latency 2.83s"
        );
    }
}
