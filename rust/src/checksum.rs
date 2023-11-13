/// Reimplementation of Sportident checksum algorithm in Rust
///
/// Note that they call it CRC but it is buggy. See the last test that leads to a checksum of 0 for
/// a polynomial that's not divisible by 0x8005.
pub fn sportident_checksum(message: &[u8]) -> u16 {
    let mut chksum: u32 = ((message[0] as u32) << 8) + message[1] as u32;
    for i in (2..message.len()).step_by(2) {
        let mut val = ((message[i] as u32) << 8) + message[i + 1] as u32;
        for _ in 0..16 {
            chksum <<= 1;
            if chksum & 0x10000 > 0 {
                if val & 0x8000 > 0 {
                    chksum += 1;
                }
                chksum ^= 0x8005;
            } else {
                if val & 0x8000 > 0 {
                    chksum += 1;
                }
            }
            val <<= 1;
        }
        chksum &= 0xffff;
    }
    chksum as u16
}

#[cfg(test)]
mod test_crc {
    use super::sportident_checksum;

    #[test]
    fn test_crc() {
        let s = b"\xd3\r\x00\x02\x00\x1f\xb5\xf3\x18\x99As\x00\x07\x08\x00";
        assert_eq!(sportident_checksum(s), 0x8f98);

        let s = b"\xd3\r\x00\x02\x00\x1f\xb5\xf3\x18\x9b\x98\x1e\x00\x070\x00";
        assert_eq!(sportident_checksum(s), 0x4428);

        let s = b"\x01\x80\x05\x00";
        assert_eq!(sportident_checksum(s), 0);
    }
}
