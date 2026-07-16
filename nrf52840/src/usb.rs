use defmt::{debug, error, info, warn};
use embassy_executor::Spawner;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::{Builder, UsbDevice};
use femtopb::Message as _;
use heapless::Vec;
use log;
use static_cell::StaticCell;

use yaroc_common::error::Error;
use yaroc_common::flash::{LoggedAtResponseIterator, MchIterator};
use yaroc_common::usb::{CdcAcm, UsbCommand, UsbDriver, UsbPacketReader, UsbResponse};

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
            usb_packet_reader_loop(SendPunchUsbPacketReader::new(self.class))
                .expect("Failed to spawn task"),
        );
        spawner.spawn(usb_logger_loop(self.log_class).expect("Failed to spawn task"));
    }
}

pub struct SendPunchUsbPacketReader<T> {
    reader: UsbPacketReader<T>,
}

impl<T: CdcAcm> SendPunchUsbPacketReader<T> {
    pub fn new(class: T) -> Self {
        Self {
            reader: UsbPacketReader::new(class),
        }
    }

    async fn write_response(&mut self, response: UsbResponse) -> Result<(), Error> {
        let response_bytes = postcard::to_vec::<_, 576>(&response)?;
        self.reader.write(response_bytes.as_slice()).await
    }

    async fn respond(&mut self, command: UsbCommand) -> Result<(), Error> {
        let mut send_punch = SEND_PUNCH_MUTEX.lock().await;
        let send_punch = send_punch.as_mut().unwrap();
        match command {
            UsbCommand::ConfigureModem(modem_config) => {
                send_punch.configure_modem(modem_config).await?;
                info!("Modem reconfigured");
                self.write_response(UsbResponse::Ok).await?;
            }
            UsbCommand::ConfigureMqtt(mqtt_config) => {
                send_punch.configure_mqtt(mqtt_config).await?;
                info!("MQTT reconfigured");
                self.write_response(UsbResponse::Ok).await?;
            }
            UsbCommand::ConfigureDevice(device_config) => {
                send_punch.update_device_config(device_config).await?;
                self.write_response(UsbResponse::Ok).await?;
            }
            UsbCommand::EraseFlash => {
                send_punch.erase_flash().await?;
                info!("Flash erased");
                self.write_response(UsbResponse::Ok).await?;
            }
            UsbCommand::GetMiniCallHomeLogs => {
                let mut iter = send_punch.get_minicallhome_logs().await?;
                loop {
                    let log = iter.next().await?;
                    match log {
                        None => break,
                        Some(mch_proto) => {
                            let mut buffer: Vec<u8, _> = Vec::new();
                            buffer
                                .resize(mch_proto.encoded_len(), 0)
                                .map_err(|_| Error::BufferTooSmallError)?;
                            mch_proto
                                .encode(&mut buffer.as_mut_slice())
                                .map_err(|_| Error::BufferTooSmallError)?;
                            self.write_response(UsbResponse::MiniCallHomeLog(buffer)).await?;
                        }
                    }
                }
                self.write_response(UsbResponse::Ok).await?;
            }
            UsbCommand::GetLoggedAtResponseLogs => {
                let mut iter = send_punch.get_logged_at_response_logs().await?;
                loop {
                    let log = iter.next().await?;
                    match log {
                        None => break,
                        Some(logged_response) => {
                            let serialized = postcard::to_vec::<_, 437>(&logged_response)?;
                            let mut vec_buffer = Vec::new();
                            vec_buffer
                                .extend_from_slice(serialized.as_slice())
                                .map_err(|_| Error::BufferTooSmallError)?;
                            self.write_response(UsbResponse::LoggedAtResponseLog(vec_buffer))
                                .await?;
                        }
                    }
                }
                self.write_response(UsbResponse::Ok).await?;
            }
        }
        Ok(())
    }

    pub async fn run(mut self) {
        loop {
            self.reader.wait_connection().await;
            info!("Connected to USB");
            loop {
                let command_result = self.reader.read().await.and_then(|data| {
                    debug!("Read {} bytes from USB", data.len());
                    postcard::from_bytes::<UsbCommand>(data).map_err(Into::into)
                });
                match command_result {
                    Ok(command) => {
                        let _ = self.respond(command).await.inspect_err(|e| {
                            error!("Error while responding to a USB command: {}", e)
                        });
                    }
                    Err(Error::UsbDisconnected) => {
                        warn!("USB disconnected");
                        break;
                    }
                    Err(e) => {
                        error!("Error while reading from USB: {}", e);
                    }
                }
            }
        }
    }
}

/// A task that reads packets from the USB and handles them.
#[embassy_executor::task]
async fn usb_packet_reader_loop(
    usb_packet_reader: SendPunchUsbPacketReader<CdcAcmClass<'static, UsbDriver>>,
) {
    usb_packet_reader.run().await;
}

#[embassy_executor::task]
async fn usb_logger_loop(log_class: CdcAcmClass<'static, UsbDriver>) {
    embassy_usb_logger::with_class!(1024, log::LevelFilter::Debug, log_class).await;
}
