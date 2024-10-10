use crate::protobufs::Timestamp;
use chrono::prelude::*;
use chrono::DateTime;

pub fn timestamp<T: TimeZone>(posix_time: i64, nanos: u32, tz: &T) -> DateTime<FixedOffset> {
    tz.timestamp_opt(posix_time, nanos).unwrap().fixed_offset()
}

#[allow(dead_code)]
fn from_proto_timestamp(time: Timestamp) -> DateTime<FixedOffset> {
    let millis = time.millis_epoch % 1000;
    let seconds = time.millis_epoch / 1000;
    timestamp(
        seconds.try_into().unwrap(),
        (millis as u32) * 1_000_000,
        &Local,
    )
}

#[allow(dead_code)]
fn current_timestamp() -> Timestamp {
    timestamp_from_datetime(Local::now())
}

pub(crate) fn timestamp_from_datetime(now: DateTime<Local>) -> Timestamp {
    Timestamp {
        millis_epoch: now.timestamp_millis().try_into().unwrap(),
    }
}

#[cfg(test)]
mod test_time {
    use crate::protobufs::Timestamp;
    use chrono::{FixedOffset, Local};

    use super::{from_proto_timestamp, timestamp, timestamp_from_datetime};

    #[test]
    fn test_timestamp() {
        let tz = FixedOffset::east_opt(3600).unwrap();
        let timestamp = timestamp(1706523131, 80234987, &tz)
            .format("%H:%M:%S.%6f")
            .to_string();
        assert_eq!("11:12:11.080234", timestamp);
    }

    #[test]
    fn test_proto_timestamp() {
        let t = Timestamp {
            millis_epoch: 1706523131124,
        };
        let timestamp = from_proto_timestamp(t)
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string();
        assert_eq!("2024-01-29 11:12:11.124", timestamp);
    }

    #[test]
    fn test_proto_timestamp_now() {
        let now = Local::now();
        let now_through_proto = from_proto_timestamp(timestamp_from_datetime(now));
        let now_formatted = now.format("%Y-%m-%d %H:%M:%S%.3f").to_string();
        assert_eq!(
            now_formatted,
            now_through_proto
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string(),
        );
    }
}
