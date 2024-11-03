#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_nrf::bind_interrupts;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::peripherals::{P1_03, P1_04, TIMER0, UARTE1};
use embassy_nrf::uarte::{self, UarteRxWithIdle};
use embassy_time::{Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    UARTE1 => uarte::InterruptHandler<UARTE1>;
});

struct Device<'a> {
    rx1: UarteRxWithIdle<'a, UARTE1, TIMER0>,
    blue_led: Output<'a, P1_04>,
    green_led: Output<'a, P1_03>,
}

impl Device<'_> {
    pub fn new() -> Self {
        let p = embassy_nrf::init(Default::default());
        let uart1 = uarte::Uarte::new(p.UARTE1, Irqs, p.P0_15, p.P0_16, Default::default());
        let (_tx1, rx1) = uart1.split_with_idle(p.TIMER0, p.PPI_CH0, p.PPI_CH1);
        let green_led = Output::new(p.P1_03, Level::Low, OutputDrive::Standard);
        let blue_led = Output::new(p.P1_04, Level::Low, OutputDrive::Standard);
        Self {
            rx1,
            blue_led,
            green_led,
        }
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let _device = Device::new();
    info!("Device initialized!");

    loop {
        info!("Blink");
        Timer::after(Duration::from_millis(100)).await;
    }
}
