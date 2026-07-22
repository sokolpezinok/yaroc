use defmt::error;
use embassy_nrf::config::Config as NrfConfig;
use embassy_nrf::gpio::{AnyPin, Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::interrupt::{Interrupt, InterruptExt, Priority};
use embassy_nrf::peripherals::{UARTE0, UARTE1};
use embassy_nrf::saadc::{ChannelConfig, Config as SaadcConfig, Saadc, Time};
use embassy_nrf::uarte::{self, UarteRxWithIdle, UarteTx};
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_nrf::usb::{self, Driver};
use embassy_nrf::{bind_interrupts, saadc, temp};
use embassy_sync::lazy_lock::LazyLock;
use embassy_sync::mutex::Mutex;
use heapless::String;
use static_cell::StaticCell;

use crate::ble::Ble;
use crate::flash::NrfFlash;
use crate::usb::Usb;
use yaroc_common::RawMutex;
use yaroc_common::at::uart::AtUart;
use yaroc_common::flash::{Flash, ValueIndex};
use yaroc_common::send_punch::{DeviceConfig, UartRxPin};
use yaroc_common::si_uart::SiUart;

use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    SAADC => saadc::InterruptHandler;
    TEMP => temp::InterruptHandler;
    UARTE0 => uarte::InterruptHandler<UARTE0>;
    UARTE1 => uarte::InterruptHandler<UARTE1>;
    USBD => usb::InterruptHandler<embassy_nrf::peripherals::USBD>;
    CLOCK_POWER => usb::vbus_detect::InterruptHandler;
});

/// A struct containing all the initialized drivers and peripherals of the device
pub struct Device {
    _blue_led: Output<'static>,
    /// Green LED
    pub green_led: Output<'static>,
    /// The MAC address of the device
    pub mac_address: String<12>,
    /// The BG77 modem driver
    pub bg77: AtUart<UarteTx<'static>, UarteRxWithIdle<'static>>,
    /// The modem PIN
    pub modem_pin: Output<'static>,
    /// The SAADC driver
    pub saadc: Saadc<'static, 1>,
    /// The SportIdent UART driver
    pub si_uart: SiUart<UarteRxWithIdle<'static>>,
    /// The Bluetooth Low Energy device
    pub ble: Ble,
    /// The NRF flash.
    pub flash: NrfFlash<'static>,
    /// The USB device
    pub usb: Usb,
    /// Device config.
    pub device_config: DeviceConfig,
}

/// The mechanism for detecting VBUS (USB power) presence.
static VBUS_DETECT: LazyLock<SoftwareVbusDetect> =
    LazyLock::new(|| SoftwareVbusDetect::new(true, true));
static FLASH_MUTEX: StaticCell<Mutex<RawMutex, nrf_softdevice::Flash>> = StaticCell::new();

impl Device {
    /// Initializes all the drivers and peripherals of the device with the given configuration
    pub async fn new() -> Self {
        let mut config: NrfConfig = Default::default();
        config.time_interrupt_priority = Priority::P2;
        let p = embassy_nrf::init(config);

        let ble = Ble::new();
        let flash_mutex = FLASH_MUTEX.init(Mutex::new(ble.flash()));
        let mut flash = NrfFlash::new(flash_mutex);
        let device_config = match flash.read::<DeviceConfig>(ValueIndex::DeviceConfig).await {
            Ok(config) => config.unwrap_or_default(),
            Err(err) => {
                error!("Error while reading device config from flash: {}", err);
                DeviceConfig::default()
            }
        };

        let mut config = uarte::Config::default();
        config.baudrate = uarte::Baudrate::Baud38400;
        Interrupt::UARTE0.set_priority(Priority::P2);
        Interrupt::UARTE1.set_priority(Priority::P2);
        let rx_pin = match device_config.srr_rx_pin {
            UartRxPin::Ain1 => p.P0_31.into::<AnyPin>(),
            UartRxPin::Scl => p.P0_14.into::<AnyPin>(),
            UartRxPin::Sda => p.P0_13.into::<AnyPin>(),
        };
        let uart0 = uarte::Uarte::new(p.UARTE0, rx_pin, p.P0_20, Irqs, config);
        let uart1 = uarte::Uarte::new(p.UARTE1, p.P0_15, p.P0_16, Irqs, Default::default());
        let (_tx0, rx0) = uart0.split_with_idle(p.TIMER2, p.PPI_CH2, p.PPI_CH3);
        let (tx1, rx1) = uart1.split_with_idle(p.TIMER1, p.PPI_CH0, p.PPI_CH1);
        let _io3 = Input::new(p.P0_21, Pull::Up);

        let modem_pin = Output::new(p.P0_17, Level::Low, OutputDrive::Standard);
        let bg77 = AtUart::new(tx1, rx1);

        let green_led = Output::new(p.P1_03, Level::Low, OutputDrive::Standard);
        let blue_led = Output::new(p.P1_04, Level::Low, OutputDrive::Standard);

        let mut saadc_config = SaadcConfig::default();
        saadc_config.resolution = saadc::Resolution::_12bit;
        saadc_config.oversample = saadc::Oversample::Over4x;
        let mut channel_config = ChannelConfig::single_ended(p.P0_05);
        channel_config.gain = saadc::Gain::Gain1_5;
        channel_config.time = Time::_40US;
        Interrupt::SAADC.set_priority(Priority::P5);
        let saadc = Saadc::new(p.SAADC, Irqs, saadc_config, [channel_config]);

        Interrupt::USBD.set_priority(Priority::P5);
        Interrupt::CLOCK_POWER.set_priority(Priority::P5);
        let driver = Driver::new(p.USBD, Irqs, VBUS_DETECT.get());
        let usb = Usb::new(driver);

        let mac_address = ble.get_mac_address();
        Self {
            _blue_led: blue_led,
            green_led,
            mac_address,
            bg77,
            modem_pin,
            si_uart: SiUart::new(rx0),
            saadc,
            ble,
            flash,
            usb,
            device_config,
        }
    }
}
