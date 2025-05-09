use crate::bg77_hw::Bg77;
use crate::si_uart::SiUart;
use embassy_nrf::config::Config as NrfConfig;
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::interrupt::{Interrupt, InterruptExt, Priority};
use embassy_nrf::peripherals::{TEMP, TIMER1, UARTE0, UARTE1};
use embassy_nrf::saadc::{ChannelConfig, Config as SaadcConfig, Saadc};
use embassy_nrf::temp;
use embassy_nrf::uarte::{self, UarteRxWithIdle, UarteTx};
use embassy_nrf::{bind_interrupts, saadc};

use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    SAADC => saadc::InterruptHandler;
    TEMP => temp::InterruptHandler;
    UARTE0 => uarte::InterruptHandler<UARTE0>;
    UARTE1 => uarte::InterruptHandler<UARTE1>;
});

#[cfg(not(feature = "bluetooth-le"))]
pub type OwnTemp = crate::system_info::NrfTemp;
#[cfg(feature = "bluetooth-le")]
pub type OwnTemp = crate::system_info::SoftdeviceTemp;

#[cfg(not(feature = "bluetooth-le"))]
fn create_temp(t: TEMP) -> crate::system_info::NrfTemp {
    let temp = embassy_nrf::temp::Temp::new(t, Irqs);
    crate::system_info::NrfTemp::new(temp)
}

#[cfg(feature = "bluetooth-le")]
fn create_temp(_: TEMP) -> crate::system_info::SoftdeviceTemp {
    crate::system_info::SoftdeviceTemp {}
}

pub struct Device {
    _blue_led: Output<'static>,
    _green_led: Output<'static>,
    pub bg77:
        Bg77<UarteTx<'static, UARTE1>, UarteRxWithIdle<'static, UARTE1, TIMER1>, Output<'static>>,
    pub saadc: Saadc<'static, 1>,
    pub si_uart: SiUart,
    pub temp: OwnTemp,
}

impl Device {
    pub fn new() -> Self {
        let mut config: NrfConfig = Default::default();
        if cfg!(feature = "bluetooth-le") {
            config.time_interrupt_priority = Priority::P2;
        }
        let mut p = embassy_nrf::init(config);
        let mut config = uarte::Config::default();
        config.baudrate = uarte::Baudrate::BAUD38400;
        Interrupt::UARTE0.set_priority(Priority::P2);
        Interrupt::UARTE1.set_priority(Priority::P2);
        // P0.14 is SCL, use it for UART0. P0.20 is TX, so it's unused.
        let uart0 = uarte::Uarte::new(p.UARTE0, Irqs, p.P0_14, p.P0_20, config);
        let uart1 = uarte::Uarte::new(p.UARTE1, Irqs, p.P0_15, p.P0_16, Default::default());
        let (_tx0, rx0) = uart0.split();
        let (tx1, rx1) = uart1.split_with_idle(p.TIMER1, p.PPI_CH0, p.PPI_CH1);
        let _io3 = Input::new(p.P0_21, Pull::Up);

        let modem_pin = Output::new(p.P0_17, Level::Low, OutputDrive::Standard);
        let bg77 = Bg77::new(tx1, rx1, modem_pin);

        let green_led = Output::new(p.P1_03, Level::Low, OutputDrive::Standard);
        let blue_led = Output::new(p.P1_04, Level::Low, OutputDrive::Standard);

        let saadc_config = SaadcConfig::default();
        let channel_config = ChannelConfig::single_ended(&mut p.P0_05);
        Interrupt::SAADC.set_priority(Priority::P5);
        let saadc = Saadc::new(p.SAADC, Irqs, saadc_config, [channel_config]);

        let temp = create_temp(p.TEMP);

        Self {
            _blue_led: blue_led,
            _green_led: green_led,
            bg77,
            temp,
            si_uart: SiUart::new(rx0),
            saadc,
        }
    }
}
