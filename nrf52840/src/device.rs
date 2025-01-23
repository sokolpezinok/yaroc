use crate::bg77_hw::Bg77;
use crate::si_uart::{SiUart, SoftwareSerial};
use crate::system_info::NrfTemp;
use cortex_m::peripheral::Peripherals as CortexMPeripherals;
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::peripherals::{TIMER0, UARTE0, UARTE1};
use embassy_nrf::saadc::{ChannelConfig, Config as SaadcConfig, Saadc};
use embassy_nrf::temp::{self, Temp};
use embassy_nrf::uarte::{self, UarteRxWithIdle, UarteTx};
use embassy_nrf::{bind_interrupts, saadc};

use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    SAADC => saadc::InterruptHandler;
    TEMP => temp::InterruptHandler;
    UARTE0 => uarte::InterruptHandler<UARTE0>;
    UARTE1 => uarte::InterruptHandler<UARTE1>;
});

pub struct Device {
    _blue_led: Output<'static>,
    _green_led: Output<'static>,
    pub bg77:
        Bg77<UarteTx<'static, UARTE1>, UarteRxWithIdle<'static, UARTE1, TIMER0>, Output<'static>>,
    pub temp: NrfTemp,
    pub si_uart: SiUart,
    pub saadc: Saadc<'static, 1>,
    pub software_serial: SoftwareSerial,
}

impl Default for Device {
    fn default() -> Self {
        Self::new()
    }
}

impl Device {
    pub fn new() -> Self {
        let mut cortex_peripherals = CortexMPeripherals::take().unwrap();
        cortex_peripherals.DCB.enable_trace();
        cortex_peripherals.DWT.enable_cycle_counter();
        cortex_peripherals.DWT.set_cycle_count(0);

        let mut p = embassy_nrf::init(Default::default());
        let mut config = uarte::Config::default();
        config.baudrate = uarte::Baudrate::BAUD38400;
        let uart0 = uarte::Uarte::new(p.UARTE0, Irqs, p.P0_19, p.P0_20, config);
        let uart1 = uarte::Uarte::new(p.UARTE1, Irqs, p.P0_15, p.P0_16, Default::default());
        let (_tx0, rx0) = uart0.split();
        let (tx1, rx1) = uart1.split_with_idle(p.TIMER0, p.PPI_CH0, p.PPI_CH1);

        let modem_pin = Output::new(p.P0_17, Level::Low, OutputDrive::Standard);
        let bg77 = Bg77::new(tx1, rx1, modem_pin);

        let green_led = Output::new(p.P1_03, Level::Low, OutputDrive::Standard);
        let blue_led = Output::new(p.P1_04, Level::Low, OutputDrive::Standard);
        let temp = Temp::new(p.TEMP, Irqs);
        let temp = NrfTemp::new(temp);

        let saadc_config = SaadcConfig::default();
        let channel_config = ChannelConfig::single_ended(&mut p.P0_05);
        let saadc = Saadc::new(p.SAADC, Irqs, saadc_config, [channel_config]);

        let io2 = Input::new(p.P1_02, Pull::Up);
        let _io3 = Input::new(p.P0_21, Pull::Up);

        Self {
            _blue_led: blue_led,
            _green_led: green_led,
            bg77,
            temp,
            si_uart: SiUart::new(rx0),
            software_serial: SoftwareSerial::new(io2),
            saadc,
        }
    }
}
