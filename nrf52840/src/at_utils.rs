pub fn readline(s: &str) -> (Option<&str>, &str) {
    match s.find("\r\n") {
        None => (None, s),
        Some(len) => (Some(&s[..len]), &s[len + 2..]),
    }
}

//#[cfg(test)]
//mod test_at_utils {
//    use crate::at_utils::readline;
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
//}
