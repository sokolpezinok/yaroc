#![no_std]
#![no_main]

use core::cell::RefCell;

use defmt::info;
use embassy_boot::{
    AlignedBuffer, BlockingFirmwareUpdater, BootLoader, BootLoaderConfig, FirmwareUpdaterConfig,
    State,
};
use embassy_nrf::config::{Config as NrfConfig, HfclkSource};
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::nvmc::{Nvmc, PAGE_SIZE};
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::{self, Driver};
use embassy_nrf::{bind_interrupts, peripherals};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_usb::Builder;
use embassy_usb_dfu::consts::DfuAttributes;
use embassy_usb_dfu::{ResetImmediate, new_state, usb_dfu};
use static_cell::StaticCell;

use {defmt_rtt as _, panic_probe as _};

#[used]
#[unsafe(link_section = ".uicr_bootloader_start_address")]
pub static BOOTLOADER_START: u32 = 0x000F_4000;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => usb::vbus_detect::InterruptHandler;
});

type FlashMutex = Mutex<NoopRawMutex, RefCell<Nvmc<'static>>>;

static FLASH: StaticCell<FlashMutex> = StaticCell::new();
static EXECUTOR: StaticCell<embassy_executor::Executor> = StaticCell::new();

#[cortex_m_rt::entry]
fn main() -> ! {
    let nvmc = unsafe { peripherals::NVMC::steal() };
    let flash = FLASH.init(Mutex::new(RefCell::new(Nvmc::new(nvmc))));
    let boot_config = BootLoaderConfig::from_linkerfile_blocking(flash, flash, flash);

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

        let mut config = NrfConfig::default();
        config.hfclk_source = HfclkSource::ExternalXtal;
        let p = embassy_nrf::init(config);

        let executor = EXECUTOR.init(embassy_executor::Executor::new());
        executor.run(|spawner| {
            spawner.spawn(dfu_task(p, flash).unwrap());
        });
    }

    info!(
        "Booting application at 0x{:08x} (state: {})",
        active_start, state
    );
    unsafe {
        cortex_m::Peripherals::steal().SCB.vtor.write(0);
        embassy_boot_nrf::BootLoader::<PAGE_SIZE>.load(active_start)
    }
}

#[embassy_executor::task]
async fn dfu_task(p: embassy_nrf::Peripherals, flash: &'static FlashMutex) -> ! {
    // Turn on both green and blue LEDs (active-high) to indicate DFU mode.
    let _green_led = Output::new(p.P1_03, Level::High, OutputDrive::Standard);
    let _blue_led = Output::new(p.P1_04, Level::High, OutputDrive::Standard);

    let updater_config = FirmwareUpdaterConfig::from_linkerfile_blocking(flash, flash);
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

#[defmt::panic_handler]
fn panic() -> ! {
    cortex_m::asm::udf()
}
