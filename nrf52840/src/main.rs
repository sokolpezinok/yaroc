#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_nrf::{
    bind_interrupts,
    peripherals::{self, TIMER0, UARTE1},
    uarte::{self, UarteRxWithIdle},
};
use embassy_time::{Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    UARTE1 => uarte::InterruptHandler<peripherals::UARTE1>;
});

struct Device<'a> {
    rx: UarteRxWithIdle<'a, UARTE1, TIMER0>,
}

impl Device<'_> {
    pub fn new() -> Self {
        let p = embassy_nrf::init(Default::default());
        let uart1 = uarte::Uarte::new(p.UARTE1, Irqs, p.P0_15, p.P0_16, Default::default());
        let (_tx, rx) = uart1.split_with_idle(p.TIMER0, p.PPI_CH0, p.PPI_CH1);
        Self { rx }
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let device = Device::new();
    info!("uarte initialized!");

    loop {
        info!("Blink");
        Timer::after(Duration::from_millis(100)).await;
    }
}
