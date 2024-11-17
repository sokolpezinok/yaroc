#![no_std]
use core::option::{Option, Option::None, Option::Some};

pub fn split_at_response(line: &str) -> Option<(&str, &str)> {
    if line.starts_with('+') {
        if let Some(prefix_len) = line.find(": ") {
            let prefix = &line[1..prefix_len];
            let rest = &line[prefix_len + 2..];
            return Some((prefix, rest));
        }
    }
    None
}

#[cfg(test)]
mod test_at_utils {
    use super::*;

    #[test]
    fn test_split_at_response() {
        let res = "+QMTSTAT: 0,2";
        assert_eq!(split_at_response(res), Some(("QMTSTAT", "0,2")));

        let res = "QMTSTAT: 0,2";
        assert_eq!(split_at_response(res), None);
        let res = "+QMTSTAT 0,2";
        assert_eq!(split_at_response(res), None);
    }
}
