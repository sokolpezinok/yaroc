use crate::at_utils::Uart;
use crate::bg77::BG77;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::peripherals::{P1_03, P1_04, UARTE1};
use embassy_nrf::saadc::{ChannelConfig, Config, Saadc};
use embassy_nrf::uarte::{self};
use embassy_nrf::{bind_interrupts, saadc};

use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    UARTE1 => uarte::InterruptHandler<UARTE1>;
    SAADC => saadc::InterruptHandler;
});

pub struct Device<'a> {
    _blue_led: Output<'a, P1_04>,
    _green_led: Output<'a, P1_03>,
    pub bg77: BG77<'a>,
    pub saadc: Saadc<'a, 1>,
}

impl<'a> Device<'a> {
    pub fn new() -> Self {
        let mut p = embassy_nrf::init(Default::default());
        let uart1 = uarte::Uarte::new(p.UARTE1, Irqs, p.P0_15, p.P0_16, Default::default());
        let (tx1, rx1) = uart1.split_with_idle(p.TIMER0, p.PPI_CH0, p.PPI_CH1);
        let uart1 = Uart::new(rx1, tx1);
        let modem_pin = Output::new(p.P0_17, Level::Low, OutputDrive::Standard);

        let green_led = Output::new(p.P1_03, Level::Low, OutputDrive::Standard);
        let blue_led = Output::new(p.P1_04, Level::Low, OutputDrive::Standard);

        let config = Config::default();
        let channel_config = ChannelConfig::single_ended(&mut p.P0_05);
        let saadc = Saadc::new(p.SAADC, Irqs, config, [channel_config]);
        Self {
            _blue_led: blue_led,
            _green_led: green_led,
            bg77: BG77::new(uart1, modem_pin),
            saadc,
        }
    }
}
