use core::str::from_utf8;
use heapless::Vec;

use crate::error::Error;

fn readline(s: &str) -> (Option<&str>, &str) {
    match s.find("\r\n") {
        None => (None, s),
        Some(len) => (Some(&s[..len]), &s[len + 2..]),
    }
}

pub fn split_lines(buf: &[u8]) -> Result<Vec<&str, 10>, Error> {
    let mut lines = Vec::new();
    let mut s = from_utf8(buf).map_err(|_| Error::StringEncodingError)?;
    while let (Some(line), rest) = readline(s) {
        s = rest;
        if line.is_empty() {
            continue;
        }
        lines.push(line).map_err(|_| Error::BufferTooSmallError)?;
    }
    Ok(lines)
}

//#[cfg(test)]
//mod test_at_utils {
//    use super::{readline, split_lines};
//
//    #[test]
//    fn test_readline() {
//        let s = "+MSG: hello\r\nOK\r\n";
//        let (l1, s) = readline(s);
//        let (l2, s) = readline(s);
//        let (l3, _) = readline(s);
//        assert_eq!(l1, Some("+MSG: hello"));
//        assert_eq!(l2, Some("OK"));
//        assert_eq!(l3, None);
//    }
//
//    #[test]
//    fn test_split_lines() {
//        let s = "hello\r\n\r\nOK\r\n";
//        let lines = split_lines(s.as_bytes());
//        assert_eq!(*lines, ["hello", "OK"]);
//    }
//}
