#[cfg(feature = "std")]
pub mod fake_modem;
#[cfg(feature = "nrf")]
pub mod nrf;
pub mod response;
pub mod uart;
