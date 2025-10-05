use crate::ble::Ble;
use crate::flash::Flash;
use crate::si_uart::SiUart;
use embassy_nrf::config::Config as NrfConfig;
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::interrupt::{Interrupt, InterruptExt, Priority};
use embassy_nrf::peripherals::{UARTE0, UARTE1};
use embassy_nrf::saadc::{ChannelConfig, Config as SaadcConfig, Saadc};
use embassy_nrf::temp;
use embassy_nrf::uarte::{self, UarteRxWithIdle, UarteTx};
use embassy_nrf::{bind_interrupts, saadc};
use heapless::String;
use yaroc_common::bg77::hw::{Bg77, ModemConfig};

use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    SAADC => saadc::InterruptHandler;
    TEMP => temp::InterruptHandler;
    UARTE0 => uarte::InterruptHandler<UARTE0>;
    UARTE1 => uarte::InterruptHandler<UARTE1>;
});

/// A struct containing all the initialized drivers and peripherals of the device
pub struct Device {
    _blue_led: Output<'static>,
    _green_led: Output<'static>,
    /// The MAC address of the device
    pub mac_address: String<12>,
    /// The BG77 modem driver
    pub bg77: Bg77<UarteTx<'static>, UarteRxWithIdle<'static>, Output<'static>>,
    /// The SAADC driver
    pub saadc: Saadc<'static, 1>,
    /// The SportIdent UART driver
    pub si_uart: SiUart<UarteRxWithIdle<'static>>,
    /// The Bluetooth Low Energy driver
    pub ble: Ble,
    /// The flash memory driver
    pub flash: Flash,
}

impl Device {
    /// Initializes all the drivers and peripherals of the device
    pub fn new(modem_config: ModemConfig) -> Self {
        let mut config: NrfConfig = Default::default();
        config.time_interrupt_priority = Priority::P2;
        let p = embassy_nrf::init(config);

        let mut config = uarte::Config::default();
        config.baudrate = uarte::Baudrate::BAUD38400;
        Interrupt::UARTE0.set_priority(Priority::P2);
        Interrupt::UARTE1.set_priority(Priority::P2);
        // P0.14 is SCL, use it for UART0. P0.20 is UART0 TX, so it's unused.
        let uart0 = uarte::Uarte::new(p.UARTE0, p.P0_14, p.P0_20, Irqs, config);
        let uart1 = uarte::Uarte::new(p.UARTE1, p.P0_15, p.P0_16, Irqs, Default::default());
        let (_tx0, rx0) = uart0.split_with_idle(p.TIMER2, p.PPI_CH2, p.PPI_CH3);
        let (tx1, rx1) = uart1.split_with_idle(p.TIMER1, p.PPI_CH0, p.PPI_CH1);
        let _io3 = Input::new(p.P0_21, Pull::Up);

        let modem_pin = Output::new(p.P0_17, Level::Low, OutputDrive::Standard);
        let bg77 = Bg77::new(tx1, rx1, modem_pin, modem_config);

        let green_led = Output::new(p.P1_03, Level::Low, OutputDrive::Standard);
        let blue_led = Output::new(p.P1_04, Level::Low, OutputDrive::Standard);

        let saadc_config = SaadcConfig::default();
        let channel_config = ChannelConfig::single_ended(p.P0_05);
        Interrupt::SAADC.set_priority(Priority::P5);
        let saadc = Saadc::new(p.SAADC, Irqs, saadc_config, [channel_config]);

        let ble = Ble::new();
        let mac_address = ble.get_mac_address();
        let flash = Flash::new(ble.flash());

        Self {
            _blue_led: blue_led,
            _green_led: green_led,
            mac_address,
            bg77,
            si_uart: SiUart::new(rx0),
            saadc,
            ble,
            flash,
        }
    }
}
