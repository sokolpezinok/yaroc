pub mod error;
#[cfg(feature = "std")]
pub mod fake_modem;
#[cfg(feature = "nrf")]
pub mod nrf;
pub mod response;
pub mod uart;

pub use error::AtError;
pub type Error = AtError;
pub type Result<T> = core::result::Result<T, AtError>;
