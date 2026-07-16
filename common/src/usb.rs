use core::future::Future;
#[cfg(feature = "nrf")]
use embassy_nrf::usb::{Driver, vbus_detect::SoftwareVbusDetect};
#[cfg(feature = "nrf")]
use embassy_usb::class::cdc_acm::CdcAcmClass;
use heapless::Vec;
use serde::{Deserialize, Serialize};

use crate::bg77::modem_manager::ModemConfig;
use crate::error::Error;
use crate::mqtt::MqttConfig;
use crate::send_punch::DeviceConfig;

#[cfg(feature = "nrf")]
/// Type alias for the USB driver.
pub type UsbDriver = Driver<'static, &'static SoftwareVbusDetect>;

#[derive(Debug, Serialize, Deserialize)]
/// Commands that can be sent over USB.
pub enum UsbCommand {
    /// Configure the modem.
    ConfigureModem(ModemConfig),
    /// Configure MQTT settings.
    ConfigureMqtt(MqttConfig),
    /// Configure device settings (MiniCallHome interval).
    ConfigureDevice(DeviceConfig),
    /// Erase the flash memory.
    EraseFlash,
    /// Get MiniCallHome logs.
    GetMiniCallHomeLogs,
}

#[derive(Debug, Serialize, Deserialize)]
/// Responses sent back over USB.
pub enum UsbResponse {
    /// Operation successful.
    Ok,
    /// MiniCallHome log.
    MiniCallHomeLog(Vec<u8, 54>),
}

/// Abstraction over the CDC ACM class.
pub trait CdcAcm {
    /// Reads a single packet into the buffer.
    fn read_packet(&mut self, buf: &mut [u8]) -> impl Future<Output = Result<usize, Error>>;
    /// Writes a single packet from the buffer.
    fn write_packet(&mut self, buf: &[u8]) -> impl Future<Output = Result<(), Error>>;
    /// Waits until the USB cable is connected.
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

    // Dropped async keyword based on https://tweedegolf.nl/en/blog/235/debloat-your-async-rust/
    fn wait_connection(&mut self) -> impl Future<Output = ()> {
        self.wait_connection()
    }
}

const PACKET_LEN: usize = 64;

/// Reads packets from USB, reconstructs messages, and dispatches them to the handler.
pub struct UsbPacketReader<T> {
    buffer: [u8; PACKET_LEN * 8],
    class: T,
}

impl<T: CdcAcm> UsbPacketReader<T> {
    /// Creates a new packet reader.
    pub fn new(class: T) -> Self {
        Self {
            buffer: [0; PACKET_LEN * 8],
            class,
        }
    }

    // Dropped async keyword based on https://tweedegolf.nl/en/blog/235/debloat-your-async-rust/
    pub fn wait_connection(&mut self) -> impl Future<Output = ()> {
        self.class.wait_connection()
    }

    /// Read ACM packet
    pub async fn read(&mut self) -> Result<&[u8], Error> {
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

    /// Write ACM packet
    pub async fn write(&mut self, buf: &[u8]) -> Result<(), Error> {
        for chunk in buf.chunks(PACKET_LEN) {
            self.class.write_packet(chunk).await?;
        }
        if buf.len().is_multiple_of(PACKET_LEN) {
            self.class.write_packet(&[]).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    extern crate std;
    use core::assert_matches;
    use embassy_futures::block_on;
    use postcard::{from_bytes, to_vec};
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

    #[test]
    fn test_write_small_packet() {
        let cdc = FakeCdcAcm::new(vec![vec![1u8; 10]]);
        let mut reader = UsbPacketReader::new(cdc);

        let small_data = [1u8; 10];
        block_on(reader.write(&small_data)).unwrap();
    }

    #[test]
    fn test_write_packet_chunking() {
        let cdc = FakeCdcAcm::new(vec![vec![2u8; 64], vec![2u8; 6]]);
        let mut reader = UsbPacketReader::new(cdc);

        let large_data = [2u8; 70];
        block_on(reader.write(&large_data)).unwrap();
    }

    #[test]
    fn test_write_packet_exact_chunking() {
        let cdc = FakeCdcAcm::new(vec![vec![2u8; 64], vec![2u8; 64], vec![]]);
        let mut reader = UsbPacketReader::new(cdc);

        let large_data = [2u8; 128];
        block_on(reader.write(&large_data)).unwrap();
    }

    #[test]
    fn test_erase_flash_serialization() {
        let command = UsbCommand::EraseFlash;
        let bytes = to_vec::<_, 8>(&command).unwrap();
        let decoded: UsbCommand = from_bytes(bytes.as_slice()).unwrap();
        assert_matches!(decoded, UsbCommand::EraseFlash);
    }
}
