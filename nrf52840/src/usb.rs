use defmt::{debug, error};
use embassy_executor::Spawner;
use embassy_nrf::usb::Driver;
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::{Builder, UsbDevice};
use postcard::from_bytes;
use serde::{Deserialize, Serialize};
use static_cell::StaticCell;
use yaroc_common::bg77::modem_manager::ModemConfig;
use yaroc_common::error::Error;

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

#[derive(Serialize, Deserialize)]
pub enum UsbCommand {
    ConfigureModem(ModemConfig),
}

#[embassy_executor::task]
async fn usb_packet_reader_loop(usb_packet_reader: UsbPacketReader) {
    usb_packet_reader.r#loop().await;
}

struct UsbPacketReader {
    buffer: [u8; PACKET_LEN * 8],
    // TODO: make CdcAcmClass into a trait
    class: CdcAcmClass<'static, UsbDriver>,
}

impl UsbPacketReader {
    async fn read(&mut self) -> crate::Result<&[u8]> {
        let total_len = self.buffer.len();
        let mut remaining = self.buffer.as_mut_slice();
        loop {
            let read_len =
                self.class.read_packet(remaining).await.map_err(|_| Error::UsbReadError)?;
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

    #[allow(dead_code)]
    async fn write(&mut self, buf: &[u8]) -> crate::Result<()> {
        for chunk in buf.chunks(PACKET_LEN) {
            self.class.write_packet(chunk).await.map_err(|_| Error::UsbWriteError)?;
        }
        Ok(())
    }

    pub async fn r#loop(mut self) {
        self.class.wait_connection().await;
        loop {
            let command = self.read().await.and_then(|data| {
                debug!("Read {} bytes from USB", data.len());
                from_bytes::<UsbCommand>(data).map_err(|_| Error::ParseError)
            });
            match command {
                Ok(_command) => {
                    let _ = SEND_PUNCH_MUTEX.lock().await;
                }
                Err(e) => {
                    error!("Error while reading from USB: {}", e);
                }
            }
        }
    }
}

impl From<CdcAcmClass<'static, UsbDriver>> for UsbPacketReader {
    fn from(class: CdcAcmClass<'static, UsbDriver>) -> Self {
        Self {
            buffer: [0; _],
            class,
        }
    }
}
