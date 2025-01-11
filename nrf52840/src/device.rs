use crate::bg77::{MqttConfig, BG77};
use crate::si_uart::SiUart;
use crate::status::NrfTemp;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::peripherals::{UARTE0, UARTE1};
use embassy_nrf::saadc::{ChannelConfig, Config as SaadcConfig, Saadc};
use embassy_nrf::temp::{self, Temp};
use embassy_nrf::uarte::{self, UarteTx};
use embassy_nrf::{bind_interrupts, saadc};

use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    SAADC => saadc::InterruptHandler;
    TEMP => temp::InterruptHandler;
    UARTE0_UART0 => uarte::InterruptHandler<UARTE0>;
    UARTE1 => uarte::InterruptHandler<UARTE1>;
});

pub struct Device {
    _blue_led: Output<'static>,
    _green_led: Output<'static>,
    pub bg77: BG77<NrfTemp, UarteTx<'static, UARTE1>, Output<'static>>,
    pub si_uart: SiUart,
    pub saadc: Saadc<'static, 1>,
}

impl Device {
    pub fn new(spawner: Spawner, mqtt_config: MqttConfig) -> Self {
        let mut p = embassy_nrf::init(Default::default());
        let uart0 = uarte::Uarte::new(p.UARTE0, Irqs, p.P0_19, p.P0_20, Default::default());
        let uart1 = uarte::Uarte::new(p.UARTE1, Irqs, p.P0_15, p.P0_16, Default::default());
        let (_tx0, rx0) = uart0.split();
        let (tx1, rx1) = uart1.split_with_idle(p.TIMER0, p.PPI_CH0, p.PPI_CH1);

        let modem_pin = Output::new(p.P0_17, Level::Low, OutputDrive::Standard);

        let green_led = Output::new(p.P1_03, Level::Low, OutputDrive::Standard);
        let blue_led = Output::new(p.P1_04, Level::Low, OutputDrive::Standard);
        let temp = Temp::new(p.TEMP, Irqs);
        let temp = NrfTemp::new(temp);

        let saadc_config = SaadcConfig::default();
        let channel_config = ChannelConfig::single_ended(&mut p.P0_05);
        let saadc = Saadc::new(p.SAADC, Irqs, saadc_config, [channel_config]);

        Self {
            _blue_led: blue_led,
            _green_led: green_led,
            bg77: BG77::new(rx1, tx1, modem_pin, temp, &spawner, mqtt_config),
            si_uart: SiUart::new(rx0),
            saadc,
        }
    }
}
