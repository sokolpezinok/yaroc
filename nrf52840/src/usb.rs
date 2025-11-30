use embassy_executor::Spawner;
use embassy_nrf::usb::Driver;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::{Builder, UsbDevice};
use static_cell::StaticCell;

type UsbDriver = Driver<'static, &'static embassy_nrf::usb::vbus_detect::SoftwareVbusDetect>;

#[embassy_executor::task]
pub async fn usb_task(mut usb: UsbDevice<'static, UsbDriver>) {
    usb.run().await;
}

#[allow(dead_code)]
pub struct Usb {
    device: UsbDevice<'static, UsbDriver>,
    class: CdcAcmClass<'static, UsbDriver>,
}

static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
static CONTROL_BUF: StaticCell<[u8; 128]> = StaticCell::new();
static MSOS_DESCRIPTOR: StaticCell<[u8; 128]> = StaticCell::new();
static STATE: StaticCell<State<'static>> = StaticCell::new();

impl Usb {
    pub fn new(driver: UsbDriver) -> Self {
        // TODO: figure out how to pick vendor and product ID
        let mut config = embassy_usb::Config::new(0xc0de, 0xcafe);
        config.manufacturer = Some("Sokol Pezinok");
        config.product = Some("Yaroc USB Serial");
        config.max_power = 500;
        config.max_packet_size_0 = 64;

        let mut builder = Builder::new(
            driver,
            config,
            CONFIG_DESCRIPTOR.init([0; _]).as_mut_slice(),
            BOS_DESCRIPTOR.init([0; _]).as_mut_slice(),
            MSOS_DESCRIPTOR.init([0; _]).as_mut_slice(),
            CONTROL_BUF.init([0; _]).as_mut_slice(),
        );

        let state = STATE.init(State::new());
        let class = CdcAcmClass::new(&mut builder, state, 64);
        let device = builder.build();

        Self { device, class }
    }

    pub fn must_spawn(self, spawner: Spawner) {
        spawner.must_spawn(usb_task(self.device));
    }
}
