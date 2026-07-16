use defmt::info;
use embassy_executor::Spawner;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::{Builder, UsbDevice};
use femtopb::Message as _;
use heapless::Vec;
use log;
use static_cell::StaticCell;

use yaroc_common::error::Error;
use yaroc_common::flash::MchIterator;
use yaroc_common::usb::{RequestHandler, UsbCommand, UsbDriver, UsbPacketReader, UsbResponse};

use crate::send_punch::SEND_PUNCH_MUTEX;

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
            usb_packet_reader_loop(UsbPacketReader::new(self.class, SendPunchHandler))
                .expect("Failed to spawn task"),
        );
        spawner.spawn(usb_logger_loop(self.log_class).expect("Failed to spawn task"));
    }
}

/// A handler for USB requests that relate to `SendPunch`.
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
            UsbCommand::ConfigureMqtt(mqtt_config) => {
                send_punch.configure_mqtt(mqtt_config).await?;
                info!("MQTT reconfigured");
            }
            UsbCommand::ConfigureDevice(device_config) => {
                send_punch.update_device_config(device_config).await?;
            }
            UsbCommand::EraseFlash => {
                send_punch.erase_flash().await?;
                info!("Flash erased");
            }
            UsbCommand::GetMiniCallHomeLogs => {
                let mut iter = send_punch.get_minicallhome_logs().await?;
                loop {
                    let log = iter.next().await?;
                    match log {
                        None => break,
                        Some(mch_proto) => {
                            let mut buffer: Vec<u8, 128> = Vec::new();
                            mch_proto
                                .encode(&mut buffer.as_mut_slice())
                                .map_err(|_| Error::BufferTooSmallError)?;
                            return Ok(UsbResponse::MiniCallHomeLog(buffer));
                        }
                    }
                }
            }
        }
        Ok(UsbResponse::Ok)
    }
}

/// A task that reads packets from the USB and handles them.
#[embassy_executor::task]
async fn usb_packet_reader_loop(
    usb_packet_reader: UsbPacketReader<CdcAcmClass<'static, UsbDriver>, SendPunchHandler>,
) {
    usb_packet_reader.run().await;
}

#[embassy_executor::task]
async fn usb_logger_loop(log_class: CdcAcmClass<'static, UsbDriver>) {
    embassy_usb_logger::with_class!(1024, log::LevelFilter::Debug, log_class).await;
}
