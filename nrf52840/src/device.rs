use crate::at_utils::Uart;
use embassy_nrf::bind_interrupts;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::peripherals::{P0_17, P1_03, P1_04, UARTE1};
use embassy_nrf::uarte::{self};
use embassy_time::Timer;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    UARTE1 => uarte::InterruptHandler<UARTE1>;
});

pub struct Device<'a> {
    blue_led: Output<'a, P1_04>,
    green_led: Output<'a, P1_03>,
    modem_pin: Output<'a, P0_17>,
    uart1: Uart<'a, UARTE1>,
}

impl<'a> Device<'a> {
    pub fn new() -> Self {
        let p = embassy_nrf::init(Default::default());
        let uart1 = uarte::Uarte::new(p.UARTE1, Irqs, p.P0_15, p.P0_16, Default::default());
        let (tx1, rx1) = uart1.split_with_idle(p.TIMER0, p.PPI_CH0, p.PPI_CH1);
        let green_led = Output::new(p.P1_03, Level::Low, OutputDrive::Standard);
        let blue_led = Output::new(p.P1_04, Level::Low, OutputDrive::Standard);
        let modem_pin = Output::new(p.P0_17, Level::Low, OutputDrive::Standard);
        Self {
            blue_led,
            green_led,
            modem_pin,
            uart1: Uart::new(rx1, tx1),
        }
    }

    async fn turn_on_bg77(&mut self) {
        self.modem_pin.set_low();
        Timer::after_millis(1000).await;
        self.modem_pin.set_high();
        Timer::after_millis(2000).await;
        self.modem_pin.set_low();
        Timer::after_millis(1000).await;
    }

    // TODO: get rid of this hack
    pub fn uart1(&mut self) -> &mut Uart<'a, UARTE1> {
        &mut self.uart1
    }
}
