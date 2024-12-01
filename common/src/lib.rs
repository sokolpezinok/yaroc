#![no_std]

#[cfg(feature = "at")]
pub mod at;
pub mod error;
pub mod punch;
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/yaroc.rs"));
}
pub mod status;

pub type Result<T> = core::result::Result<T, error::Error>;
