#![no_std]

pub mod at;
pub mod backoff;
pub mod bg77;
pub mod error;
pub mod punch;
pub mod send_punch;
pub mod si_uart;
pub mod status;
pub mod usb;

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/yaroc.rs"));
}

pub type Result<T> = core::result::Result<T, error::Error>;

#[cfg(all(target_abi = "eabihf", target_os = "none"))]
pub type RawMutex = embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
#[cfg(not(all(target_abi = "eabihf", target_os = "none")))]
pub type RawMutex = embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

pub const PUNCH_EXTRA_LEN: usize = 2;

#[cfg(test)]
mod test_proto {
    use crate::{
        PUNCH_EXTRA_LEN,
        proto::{MiniCallHome, Punches},
    };
    use femtopb::{Message, Repeated};

    #[test]
    fn negative_encoding() {
        let mch = MiniCallHome {
            rsrp_dbm: -1,
            signal_snr_cb: -60,
            ..Default::default()
        };
        let len = mch.encoded_len();
        assert_eq!(len, 4);
    }

    #[test]
    fn punch_encoding_length() {
        let punch1 = b"\x01\x23\x45";
        let punch2 = b"\xab\xcd\xef";
        let punch_slice: &[&[u8]] = &[punch1, punch2];

        let punches = Punches {
            punches: Repeated::from_slice(punch_slice),
            ..Default::default()
        };
        assert_eq!(punches.encoded_len(), 3 + 3 + PUNCH_EXTRA_LEN * 2);
    }
}
