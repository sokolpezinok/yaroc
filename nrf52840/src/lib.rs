#![no_std]
#![no_main]

pub mod at_utils;
pub mod bg77;
pub mod device;
pub mod error;
pub mod si_uart;

type Result<T> = core::result::Result<T, error::Error>;
