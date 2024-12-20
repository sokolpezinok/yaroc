use chrono::prelude::*;
use crate::proto::Timestamp;

pub fn datetime_from_timestamp<T: TimeZone>(timestamp: Timestamp, tz: &T) -> DateTime<FixedOffset> {
    tz.timestamp_millis_opt(timestamp.millis_epoch as i64).unwrap().fixed_offset()
}

pub fn datetime_from_millis<T: TimeZone>(timestamp: i64, tz: &T) -> DateTime<FixedOffset> {
    tz.timestamp_millis_opt(timestamp).unwrap().fixed_offset()
}

#[cfg(test)]
mod test_time {
    use chrono::{FixedOffset, Local};
    use crate::proto::Timestamp;

    use super::datetime_from_timestamp;

    #[test]
    fn test_timestamp() {
        let tz = FixedOffset::east_opt(3600).unwrap();
        let timestamp = Timestamp {
            millis_epoch: 1706523131_081,
            ..Default::default()
        };
        let timestamp = datetime_from_timestamp(timestamp, &tz).format("%H:%M:%S.%3f").to_string();
        assert_eq!("11:12:11.081", timestamp);
    }

    #[test]
    fn test_proto_timestamp() {
        let tz = FixedOffset::east_opt(3600).unwrap();
        let timestamp = Timestamp {
            millis_epoch: 1706523131_124,
            ..Default::default()
        };
        let timestamp = datetime_from_timestamp(timestamp, &tz)
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string();
        assert_eq!("2024-01-29 11:12:11.124", timestamp);
    }

    #[test]
    fn test_proto_timestamp_now() {
        let tz = FixedOffset::east_opt(3600).unwrap();
        let now = Local::now().with_timezone(&tz);
        let timestamp = Timestamp {
            millis_epoch: now.timestamp_millis() as u64,
            ..Default::default()
        };
        let now_through_proto = datetime_from_timestamp(timestamp, &tz);
        let now_formatted = now.format("%Y-%m-%d %H:%M:%S%.3f").to_string();
        assert_eq!(
            now_formatted,
            now_through_proto.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        );
    }
}
