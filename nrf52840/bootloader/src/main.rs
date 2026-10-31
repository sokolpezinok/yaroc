#![no_std]
#![no_main]

use core::cell::RefCell;

use cortex_m::peripheral::NVIC;
use defmt::info;
use embassy_boot::{
    AlignedBuffer, BlockingFirmwareUpdater, BootLoader, BootLoaderConfig, FirmwareUpdaterConfig,
    State,
};
use embassy_executor::Spawner;
use embassy_nrf::config::{Config as NrfConfig, HfclkSource};
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::interrupt::Interrupt;
use embassy_nrf::nvmc::{Nvmc, PAGE_SIZE};
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::{self, Driver};
use embassy_nrf::{bind_interrupts, peripherals};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_usb::Builder;
use embassy_usb_dfu::consts::DfuAttributes;
use embassy_usb_dfu::{ResetImmediate, new_state, usb_dfu};

use {defmt_rtt as _, panic_probe as _};

#[used]
#[unsafe(link_section = ".uicr_bootloader_start_address")]
pub static BOOTLOADER_START: u32 = 0x000F_4000;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => usb::vbus_detect::InterruptHandler;
});

#[embassy_executor::main]
async fn main(_spawner: Spawner) -> ! {
    let cp = unsafe { cortex_m::Peripherals::steal() };
    unsafe { cp.SCB.vtor.write(BOOTLOADER_START) };

    let mut config = NrfConfig::default();
    config.hfclk_source = HfclkSource::ExternalXtal;
    let p = embassy_nrf::init(config);

    let flash = Mutex::<NoopRawMutex, _>::new(RefCell::new(Nvmc::new(p.NVMC)));
    let boot_config = BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);

    let mut page_buf = AlignedBuffer([0; PAGE_SIZE]);
    let mut boot = BootLoader::new(boot_config);
    let state = boot.prepare_boot(page_buf.as_mut()).expect("Boot prepare error");

    unsafe extern "C" {
        static __bootloader_active_start: u32;
    }
    let active_start = &raw const __bootloader_active_start as u32;
    let active_present = unsafe { (active_start as *const u32).read_volatile() != 0xFFFF_FFFF };

    // TODO: Detect double-click on hardware reset button (via POWER.GPREGRET) to manually force DFU mode.
    if state == State::DfuDetach || !active_present {
        info!("Entering USB DFU bootloader mode (state: {})", state);

        // Turn on both green and blue LEDs (active-low) to indicate DFU mode.
        let _green_led = Output::new(p.P1_03, Level::Low, OutputDrive::Standard);
        let _blue_led = Output::new(p.P1_04, Level::Low, OutputDrive::Standard);

        let updater_config = FirmwareUpdaterConfig::from_linkerfile_blocking(&flash, &flash);
        let mut magic_buf = AlignedBuffer([0; 4]);
        let updater = BlockingFirmwareUpdater::new(updater_config, magic_buf.as_mut());

        let driver = Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));
        let mut usb_config = embassy_usb::Config::new(0xc0de, 0xcafe);
        usb_config.manufacturer = Some("Sokol Pezinok");
        usb_config.product = Some("Yaroc DFU Bootloader");

        let mut config_descriptor = [0; 64];
        let mut bos_descriptor = [0; 32];
        let mut msos_descriptor = [0; 64];
        let mut control_buf = [0; PAGE_SIZE];

        let mut dfu_state = new_state::<_, _, _, PAGE_SIZE>(
            updater,
            DfuAttributes::CAN_DOWNLOAD | DfuAttributes::WILL_DETACH,
            ResetImmediate,
        );

        let mut builder = Builder::new(
            driver,
            usb_config,
            &mut config_descriptor,
            &mut bos_descriptor,
            &mut msos_descriptor,
            &mut control_buf,
        );

        usb_dfu(&mut builder, &mut dfu_state, |_| {});

        let mut dev = builder.build();
        dev.run().await
    }

    info!("Booting application at 0x{:08x} (state: {})", active_start, state);
    NVIC::mask(Interrupt::RTC1);
    unsafe {
        cp.SCB.vtor.write(0);
        embassy_boot_nrf::BootLoader::<PAGE_SIZE>.load(active_start)
    }
}

#[defmt::panic_handler]
fn panic() -> ! {
    cortex_m::asm::udf()
}
