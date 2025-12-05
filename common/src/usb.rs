use core::future::Future;
#[cfg(feature = "defmt")]
use defmt::{debug, error, info, warn};
#[cfg(feature = "nrf")]
use embassy_nrf::usb::{Driver, vbus_detect::SoftwareVbusDetect};
#[cfg(feature = "nrf")]
use embassy_usb::class::cdc_acm::CdcAcmClass;
#[cfg(not(feature = "defmt"))]
use log::{debug, error, info, warn};
use postcard::{from_bytes, to_vec};
use serde::{Deserialize, Serialize};

use crate::bg77::modem_manager::ModemConfig;
use crate::bg77::mqtt::MqttConfig;
use crate::error::Error;

#[cfg(feature = "nrf")]
pub type UsbDriver = Driver<'static, &'static SoftwareVbusDetect>;

#[derive(Serialize, Deserialize)]
pub enum UsbCommand {
    ConfigureModem(ModemConfig),
    ConfigureMqtt(MqttConfig),
}

#[derive(Serialize, Deserialize)]
pub enum UsbResponse {
    Ok,
}

pub trait CdcAcm {
    fn read_packet(&mut self, buf: &mut [u8]) -> impl Future<Output = Result<usize, Error>>;
    fn write_packet(&mut self, buf: &[u8]) -> impl Future<Output = Result<(), Error>>;
    fn wait_connection(&mut self) -> impl Future<Output = ()>;
}

#[cfg(feature = "nrf")]
impl CdcAcm for CdcAcmClass<'static, UsbDriver> {
    async fn read_packet(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        use embassy_usb::driver::EndpointError;
        self.read_packet(buf).await.map_err(|e| match e {
            EndpointError::BufferOverflow => panic!("Buffer overflow"),
            EndpointError::Disabled => Error::UsbDisconnected,
        })
    }

    async fn write_packet(&mut self, buf: &[u8]) -> Result<(), Error> {
        use embassy_usb::driver::EndpointError;
        self.write_packet(buf).await.map_err(|e| match e {
            EndpointError::BufferOverflow => panic!("Buffer overflow"),
            EndpointError::Disabled => Error::UsbDisconnected,
        })
    }

    async fn wait_connection(&mut self) {
        self.wait_connection().await
    }
}

pub trait RequestHandler {
    fn handle(&mut self, command: UsbCommand) -> impl Future<Output = Result<UsbResponse, Error>>;
}

const PACKET_LEN: usize = 64;

pub struct UsbPacketReader<T, H> {
    buffer: [u8; PACKET_LEN * 8],
    class: T,
    handler: H,
}

impl<T: CdcAcm, H: RequestHandler> UsbPacketReader<T, H> {
    pub fn new(class: T, handler: H) -> Self {
        Self {
            buffer: [0; PACKET_LEN * 8],
            class,
            handler,
        }
    }

    async fn read(&mut self) -> Result<&[u8], Error> {
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

    async fn write(&mut self, buf: &[u8]) -> Result<(), Error> {
        for chunk in buf.chunks(PACKET_LEN) {
            self.class.write_packet(chunk).await?;
        }
        if buf.len().is_multiple_of(PACKET_LEN) {
            self.class.write_packet(&[]).await?;
        }
        Ok(())
    }

    async fn respond(&mut self, command: UsbCommand) -> Result<(), Error> {
        let response = self.handler.handle(command).await?;
        let response_bytes = to_vec::<_, 128>(&response)?;
        self.write(response_bytes.as_slice()).await
    }

    pub async fn run(mut self) {
        loop {
            self.class.wait_connection().await;
            info!("Connected to USB");
            loop {
                let command_result = self.read().await.and_then(|data| {
                    debug!("Read {} bytes from USB", data.len());
                    from_bytes::<UsbCommand>(data).map_err(Into::into)
                });
                match command_result {
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

#[cfg(test)]
mod tests {
    extern crate std;
    use embassy_futures::block_on;

    use super::*;
    use std::vec;
    use std::vec::Vec;

    #[derive(Default)]
    struct FakeCdcAcm {
        packets: Vec<Vec<u8>>,
    }

    impl FakeCdcAcm {
        pub fn new(packets: Vec<Vec<u8>>) -> Self {
            Self { packets }
        }
    }

    impl CdcAcm for FakeCdcAcm {
        async fn read_packet(&mut self, _buf: &mut [u8]) -> Result<usize, Error> {
            core::future::pending().await
        }

        async fn write_packet(&mut self, buf: &[u8]) -> Result<(), Error> {
            assert_eq!(self.packets[0], buf);
            self.packets.remove(0);
            Ok(())
        }

        async fn wait_connection(&mut self) {
            core::future::pending().await
        }
    }

    impl Drop for FakeCdcAcm {
        fn drop(&mut self) {
            assert!(self.packets.is_empty())
        }
    }

    struct MockRequestHandler;

    impl RequestHandler for MockRequestHandler {
        async fn handle(&mut self, _command: UsbCommand) -> Result<UsbResponse, Error> {
            core::future::pending().await
        }
    }

    #[test]
    fn test_write_small_packet() {
        let cdc = FakeCdcAcm::new(vec![vec![1u8; 10]]);
        let handler = MockRequestHandler;
        let mut reader = UsbPacketReader::new(cdc, handler);

        let small_data = [1u8; 10];
        block_on(reader.write(&small_data)).unwrap();
    }

    #[test]
    fn test_write_packet_chunking() {
        let cdc = FakeCdcAcm::new(vec![vec![2u8; 64], vec![2u8; 6]]);
        let handler = MockRequestHandler;
        let mut reader = UsbPacketReader::new(cdc, handler);

        let large_data = [2u8; 70];
        block_on(reader.write(&large_data)).unwrap();
    }

    #[test]
    fn test_write_packet_exact_chunking() {
        let cdc = FakeCdcAcm::new(vec![vec![2u8; 64], vec![2u8; 64], vec![]]);
        let handler = MockRequestHandler;
        let mut reader = UsbPacketReader::new(cdc, handler);

        let large_data = [2u8; 128];
        block_on(reader.write(&large_data)).unwrap();
    }
}
