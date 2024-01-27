use std::time::Duration;

use chrono::prelude::*;

use crate::status::Position;

#[allow(dead_code)]
struct LogMessage {
    name: String,
    timestamp: chrono::DateTime<Local>,
    latency: Duration,
    position: Option<Position>,
    dbm: Option<i16>,
    cell: Option<u32>,
    snr: Option<f32>,
}
