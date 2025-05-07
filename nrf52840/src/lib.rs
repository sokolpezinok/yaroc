#![no_std]
#![no_main]

pub mod bg77_hw;
#[cfg(feature = "bluetooth-le")]
pub mod ble;
pub mod device;
pub mod mqtt;
pub mod send_punch;
pub mod si_uart;
pub mod system_info;

pub use yaroc_common::error;
type Result<T> = yaroc_common::Result<T>;

use cortex_m_semihosting::debug;
use defmt_rtt as _;
use panic_probe as _;

#[defmt::panic_handler]
fn panic() -> ! {
    cortex_m::asm::udf()
}

/// Terminates the application and makes a semihosting-capable debug tool exit
/// with status code 0.
pub fn exit() -> ! {
    loop {
        debug::exit(debug::EXIT_SUCCESS);
    }
}

/// Hardfault handler.
///
/// Terminates the application and makes a semihosting-capable debug tool exit
/// with an error. This seems better than the default, which is to spin in a
/// loop.
#[cortex_m_rt::exception]
unsafe fn HardFault(_frame: &cortex_m_rt::ExceptionFrame) -> ! {
    loop {
        debug::exit(debug::EXIT_FAILURE);
    }
}
