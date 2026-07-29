use embassy_executor::Spawner;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::{Builder, UsbDevice};
use static_cell::StaticCell;

use yaroc_common::usb::{SendPunchUsb, UsbDriver};

use crate::flash::NrfFlash;
use crate::send_punch::{Bg77Type, FLASH_MUTEX, SEND_PUNCH_MUTEX};

/// The main USB task.
///
/// This task manages the USB device and must be spawned for USB to work.
#[embassy_executor::task]
pub async fn usb_task(mut usb: UsbDevice<'static, UsbDriver>) {
    usb.run().await;
}

static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
static CONTROL_BUF: StaticCell<[u8; 128]> = StaticCell::new();
static MSOS_DESCRIPTOR: StaticCell<[u8; 128]> = StaticCell::new();
static MAIN_STATE: StaticCell<State<'static>> = StaticCell::new();
static LOG_STATE: StaticCell<State<'static>> = StaticCell::new();
const PACKET_LEN: usize = 64;

fn builder(driver: UsbDriver) -> Builder<'static, UsbDriver> {
    // TODO: figure out how to pick vendor and product ID
    let mut config = embassy_usb::Config::new(0xc0de, 0xcafe);
    config.manufacturer = Some("Sokol Pezinok");
    config.product = Some("Yaroc USB Serial");
    config.max_packet_size_0 = 64;

    // Required for dual CDC ACM (composite device with Interface Association Descriptors)
    config.device_class = 0xef;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;
    config.composite_with_iads = true;

    Builder::new(
        driver,
        config,
        CONFIG_DESCRIPTOR.init([0; _]).as_mut_slice(),
        BOS_DESCRIPTOR.init([0; _]).as_mut_slice(),
        MSOS_DESCRIPTOR.init([0; _]).as_mut_slice(),
        CONTROL_BUF.init([0; _]).as_mut_slice(),
    )
}

/// A wrapper around the USB device and class.
pub struct Usb {
    device: UsbDevice<'static, UsbDriver>,
    class: CdcAcmClass<'static, UsbDriver>,
    log_class: CdcAcmClass<'static, UsbDriver>,
}

impl Usb {
    /// Creates a new `Usb` instance.
    pub fn new(driver: UsbDriver) -> Self {
        let mut builder = builder(driver);
        let state = MAIN_STATE.init(State::new());
        let logger_state = LOG_STATE.init(State::new());
        let main_class = CdcAcmClass::new(&mut builder, state, PACKET_LEN as u16);
        let log_class = CdcAcmClass::new(&mut builder, logger_state, PACKET_LEN as u16);
        let device = builder.build();

        Self {
            device,
            class: main_class,
            log_class,
        }
    }

    /// Spawns the USB tasks.
    ///
    /// This spawns `usb_task()` and `usb_packet_reader_loop()`.
    pub fn spawn(self, spawner: Spawner) {
        spawner.spawn(usb_task(self.device).expect("Failed to spawn task"));
        spawner.spawn(
            usb_packet_reader_loop(SendPunchUsb::new(
                self.class,
                &SEND_PUNCH_MUTEX,
                &FLASH_MUTEX,
            ))
            .expect("Failed to spawn task"),
        );
        spawner.spawn(usb_logger_loop(self.log_class).expect("Failed to spawn task"));
    }
}

type SendPunchUsbType = SendPunchUsb<CdcAcmClass<'static, UsbDriver>, Bg77Type, NrfFlash<'static>>;

/// A task that reads packets from the USB and handles them.
#[embassy_executor::task]
async fn usb_packet_reader_loop(usb_packet_reader: SendPunchUsbType) {
    usb_packet_reader.run().await;
}

#[embassy_executor::task]
async fn usb_logger_loop(log_class: CdcAcmClass<'static, UsbDriver>) {
    embassy_usb_logger::with_class!(1024, log::LevelFilter::Debug, log_class).await;
}
