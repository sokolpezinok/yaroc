#![no_std]

pub mod at;
pub mod backoff;
pub mod bg77;
pub mod error;
#[cfg(feature = "receive")]
pub mod logs;
#[cfg(feature = "receive")]
pub mod meshtastic;
pub mod punch;
#[cfg(feature = "receive")]
pub mod receive;
pub mod status;
#[cfg(feature = "std")]
pub mod system_info;

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/yaroc.rs"));
}

pub type Result<T> = core::result::Result<T, error::Error>;

#[cfg(all(target_abi = "eabihf", target_os = "none"))]
pub type RawMutex = embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
#[cfg(not(all(target_abi = "eabihf", target_os = "none")))]
pub type RawMutex = embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

#[cfg(test)]
mod test_proto {
    use crate::proto::MiniCallHome;
    use femtopb::Message;

    #[test]
    fn negative_encoding() {
        let mch = MiniCallHome {
            signal_dbm: -1,
            signal_snr_cb: -60,
            ..Default::default()
        };
        let len = mch.encoded_len();
        assert_eq!(len, 4);
    }
}
