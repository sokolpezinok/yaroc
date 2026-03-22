use defmt::info;
use embassy_executor::Spawner;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::{Builder, UsbDevice};
use static_cell::StaticCell;
use yaroc_common::error::Error;
use yaroc_common::usb::{RequestHandler, UsbCommand, UsbDriver, UsbPacketReader, UsbResponse};

use crate::send_punch::SEND_PUNCH_MUTEX;

#[embassy_executor::task]
pub async fn usb_task(mut usb: UsbDevice<'static, UsbDriver>) {
    usb.run().await;
}

static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
static CONTROL_BUF: StaticCell<[u8; 128]> = StaticCell::new();
static MSOS_DESCRIPTOR: StaticCell<[u8; 128]> = StaticCell::new();
static STATE: StaticCell<State<'static>> = StaticCell::new();
const PACKET_LEN: usize = 64;

fn builder(driver: UsbDriver) -> Builder<'static, UsbDriver> {
    // TODO: figure out how to pick vendor and product ID
    let mut config = embassy_usb::Config::new(0xc0de, 0xcafe);
    config.manufacturer = Some("Sokol Pezinok");
    config.product = Some("Yaroc USB Serial");
    config.max_power = 500;
    config.max_packet_size_0 = 64;
    config.self_powered = true;

    Builder::new(
        driver,
        config,
        CONFIG_DESCRIPTOR.init([0; _]).as_mut_slice(),
        BOS_DESCRIPTOR.init([0; _]).as_mut_slice(),
        MSOS_DESCRIPTOR.init([0; _]).as_mut_slice(),
        CONTROL_BUF.init([0; _]).as_mut_slice(),
    )
}

pub struct Usb {
    device: UsbDevice<'static, UsbDriver>,
    class: CdcAcmClass<'static, UsbDriver>,
}

impl Usb {
    pub fn new(driver: UsbDriver) -> Self {
        let mut builder = builder(driver);
        let state = STATE.init(State::new());
        let class = CdcAcmClass::new(&mut builder, state, PACKET_LEN as u16);
        let device = builder.build();

        Self { device, class }
    }

    pub fn spawn(self, spawner: Spawner) {
        spawner.spawn(usb_task(self.device).expect("Failed to spawn task"));
        spawner.spawn(
            usb_packet_reader_loop(UsbPacketReader::new(self.class, SendPunchHandler))
                .expect("Failed to spawn task"),
        );
    }
}

pub struct SendPunchHandler;

impl RequestHandler for SendPunchHandler {
    async fn handle(&mut self, command: UsbCommand) -> Result<UsbResponse, Error> {
        let mut send_punch = SEND_PUNCH_MUTEX.lock().await;
        let send_punch = send_punch.as_mut().unwrap();
        match command {
            UsbCommand::ConfigureModem(modem_config) => {
                send_punch.configure_modem(modem_config).await?;
                info!("Modem reconfigured");
            }
            UsbCommand::ConfigureMqtt(_mqtt_config) => {
                todo!();
            }
        }
        Ok(UsbResponse::Ok)
    }
}

#[embassy_executor::task]
async fn usb_packet_reader_loop(
    usb_packet_reader: UsbPacketReader<CdcAcmClass<'static, UsbDriver>, SendPunchHandler>,
) {
    usb_packet_reader.run().await;
}
