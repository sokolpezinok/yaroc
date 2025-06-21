use chrono::prelude::*;

pub fn datetime_from_secs(timestamp: i64, tz: &impl TimeZone) -> DateTime<FixedOffset> {
    tz.timestamp_opt(timestamp, 0).unwrap().fixed_offset()
}

#[cfg(feature = "std")]
#[cfg(test)]
mod test_time {
    use super::*;

    extern crate alloc;
    use alloc::string::ToString;

    #[test]
    fn test_timestamp() {
        let tz = FixedOffset::east_opt(3600).unwrap();
        let timestamp = datetime_from_secs(1706523131, &tz).format("%H:%M:%S.%3f").to_string();
        assert_eq!("11:12:11.000", timestamp);
    }

    #[test]
    fn test_proto_timestamp_now() {
        let tz = FixedOffset::east_opt(3600).unwrap();
        let now = Local::now().with_timezone(&tz);
        let now_through_proto = datetime_from_secs(now.timestamp(), &tz);
        let now_formatted = now.format("%Y-%m-%d %H:%M:%S").to_string();
        assert_eq!(
            now_formatted,
            now_through_proto.format("%Y-%m-%d %H:%M:%S").to_string(),
        );
    }
}
