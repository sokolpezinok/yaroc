use crate::protobufs::Timestamp;
use chrono::prelude::*;
use chrono::DateTime;

pub fn datetime_from_timestamp<T: TimeZone>(posix_millis: u64, tz: &T) -> DateTime<FixedOffset> {
    tz.timestamp_millis_opt(posix_millis.try_into().unwrap())
        .unwrap()
        .fixed_offset()
}

pub fn current_timestamp() -> Timestamp {
    timestamp_from_datetime(Local::now().fixed_offset())
}

pub(crate) fn timestamp_from_datetime(now: DateTime<FixedOffset>) -> Timestamp {
    Timestamp {
        millis_epoch: now.timestamp_millis().try_into().unwrap(),
    }
}

#[cfg(test)]
mod test_time {
    use crate::protobufs::Timestamp;
    use chrono::{FixedOffset, Local};

    use super::{datetime_from_timestamp, timestamp_from_datetime};

    #[test]
    fn test_timestamp() {
        let tz = FixedOffset::east_opt(3600).unwrap();
        let timestamp = datetime_from_timestamp(1706523131_081, &tz)
            .format("%H:%M:%S.%3f")
            .to_string();
        assert_eq!("11:12:11.081", timestamp);
    }

    #[test]
    fn test_proto_timestamp() {
        let tz = FixedOffset::east_opt(3600).unwrap();
        let t = Timestamp {
            millis_epoch: 1706523131_124,
        };
        let timestamp = datetime_from_timestamp(t.millis_epoch, &tz)
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string();
        assert_eq!("2024-01-29 11:12:11.124", timestamp);
    }

    #[test]
    fn test_proto_timestamp_now() {
        let tz = FixedOffset::east_opt(3600).unwrap();
        let now = Local::now().with_timezone(&tz);
        let now_through_proto =
            datetime_from_timestamp(timestamp_from_datetime(now).millis_epoch, &tz);
        let now_formatted = now.format("%Y-%m-%d %H:%M:%S%.3f").to_string();
        assert_eq!(
            now_formatted,
            now_through_proto
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string(),
        );
    }
}
