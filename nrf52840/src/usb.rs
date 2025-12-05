use defmt::{debug, error, info, warn};
use embassy_executor::Spawner;
use embassy_nrf::usb::Driver;
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::driver::EndpointError;
use embassy_usb::{Builder, UsbDevice};
use postcard::{from_bytes, to_vec};
use static_cell::StaticCell;
use yaroc_common::error::Error;
use yaroc_common::usb::{UsbCommand, UsbResponse};

use crate::send_punch::SEND_PUNCH_MUTEX;

type UsbDriver = Driver<'static, &'static SoftwareVbusDetect>;

#[embassy_executor::task]
pub async fn usb_task(mut usb: UsbDevice<'static, UsbDriver>) {
    usb.run().await;
}

pub struct Usb {
    device: UsbDevice<'static, UsbDriver>,
    class: CdcAcmClass<'static, UsbDriver>,
}

static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
static CONTROL_BUF: StaticCell<[u8; 128]> = StaticCell::new();
static MSOS_DESCRIPTOR: StaticCell<[u8; 128]> = StaticCell::new();
static STATE: StaticCell<State<'static>> = StaticCell::new();
const PACKET_LEN: usize = 64;

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
        let class = CdcAcmClass::new(&mut builder, state, PACKET_LEN as u16);
        let device = builder.build();

        Self { device, class }
    }

    pub fn must_spawn(self, spawner: Spawner) {
        spawner.must_spawn(usb_task(self.device));
        spawner.must_spawn(usb_packet_reader_loop(UsbPacketReader::from(self.class)));
    }
}

pub trait CdcAcm {
    fn read_packet(&mut self, buf: &mut [u8]) -> impl Future<Output = crate::Result<usize>>;
    fn write_packet(&mut self, buf: &[u8]) -> impl Future<Output = crate::Result<()>>;
    fn wait_connection(&mut self) -> impl Future<Output = ()>;
}

impl<'d, D: embassy_usb::driver::Driver<'d>> CdcAcm for CdcAcmClass<'d, D> {
    async fn read_packet(&mut self, buf: &mut [u8]) -> crate::Result<usize> {
        self.read_packet(buf).await.map_err(|e| match e {
            EndpointError::BufferOverflow => panic!("Buffer overflow"),
            EndpointError::Disabled => Error::UsbDisconnected,
        })
    }

    async fn write_packet(&mut self, buf: &[u8]) -> crate::Result<()> {
        self.write_packet(buf).await.map_err(|e| match e {
            EndpointError::BufferOverflow => panic!("Buffer overflow"),
            EndpointError::Disabled => Error::UsbDisconnected,
        })
    }

    async fn wait_connection(&mut self) {
        self.wait_connection().await
    }
}

#[embassy_executor::task]
async fn usb_packet_reader_loop(
    usb_packet_reader: UsbPacketReader<CdcAcmClass<'static, UsbDriver>>,
) {
    usb_packet_reader.r#loop().await;
}

struct UsbPacketReader<T> {
    buffer: [u8; PACKET_LEN * 8],
    class: T,
}

impl<T: CdcAcm> UsbPacketReader<T> {
    async fn read(&mut self) -> crate::Result<&[u8]> {
        let total_len = self.buffer.len();
        let mut remaining = self.buffer.as_mut_slice();
        loop {
            let read_len = self.class.read_packet(remaining).await?;
            match read_len {
                PACKET_LEN => {
                    remaining = &mut remaining[PACKET_LEN..];
                }
                len => {
                    let len = total_len - remaining.len() + len;
                    return Ok(&self.buffer[..len]);
                }
            }
        }
    }

    async fn write(&mut self, buf: &[u8]) -> crate::Result<()> {
        for chunk in buf.chunks(PACKET_LEN) {
            self.class.write_packet(chunk).await.map_err(|_| Error::UsbWriteError)?;
        }
        // TODO: if PACKET_LEN divides buf.len(), we need to send an empty packet.
        Ok(())
    }

    async fn respond(&mut self, command: UsbCommand) -> crate::Result<()> {
        let mut send_punch = SEND_PUNCH_MUTEX.lock().await;
        let send_punch = send_punch.as_mut().unwrap();
        match command {
            UsbCommand::ConfigureModem(modem_config) => {
                info!("Will configure modem now");
                send_punch.configure_modem(modem_config).await?;
            }
        }
        let response = to_vec::<_, 128>(&UsbResponse::Ok).unwrap();
        self.write(response.as_slice()).await
    }

    pub async fn r#loop(mut self) {
        loop {
            self.class.wait_connection().await;
            info!("Connected to USB");
            loop {
                let command = self.read().await.and_then(|data| {
                    debug!("Read {} bytes from USB", data.len());
                    from_bytes::<UsbCommand>(data).map_err(|_| Error::ParseError)
                });
                match command {
                    Ok(command) => {
                        let _ = self
                            .respond(command)
                            .await
                            .inspect_err(|_| error!("Error while responding to a USB command"));
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

impl<T> From<T> for UsbPacketReader<T> {
    fn from(class: T) -> Self {
        Self {
            buffer: [0; _],
            class,
        }
    }
}
