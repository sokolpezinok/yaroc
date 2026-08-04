use core::future::Future;
#[cfg(feature = "nrf")]
use embassy_nrf::usb::{Driver, vbus_detect::SoftwareVbusDetect};
use embassy_sync::mutex::Mutex;
#[cfg(feature = "nrf")]
use embassy_usb::class::cdc_acm::CdcAcmClass;
use femtopb::Message as _;
use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

#[cfg(feature = "defmt")]
use defmt::{debug, error, info, warn};
#[cfg(not(feature = "defmt"))]
use log::{debug, error, info, warn};

use crate::RawMutex;
use crate::bg77::modem::Modem;
use crate::bg77::modem_manager::ModemConfig;
use crate::error::Error;
use crate::flash::{Flash, FlashGuard, LoggedAtResponseIterator, MchIterator};
use crate::mqtt::MqttConfig;
use crate::send_punch::{DeviceConfig, SendPunch};

#[cfg(feature = "nrf")]
/// Type alias for the USB driver.
pub type UsbDriver = Driver<'static, &'static SoftwareVbusDetect>;

/// Protocol version for USB handshake.
pub const PROTOCOL_VERSION: u32 = 0;

#[derive(Debug, Serialize, Deserialize)]
/// Commands that can be sent over USB.
pub enum UsbCommand {
    /// Handshake command to identify device and protocol version.
    Handshake,
    /// Read all configs from flash.
    GetConfig,
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
    /// Get LoggedAtResponse logs.
    GetLoggedAtResponseLogs,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
/// Responses sent back over USB.
#[allow(clippy::large_enum_variant)]
pub enum UsbResponse {
    /// Handshake response containing magic string and protocol version.
    Handshake(String<8>, u32),
    /// Operation successful.
    Ok,
    /// Partial success with expected next operation timeout in milliseconds.
    PartialOk(u32),
    /// Stored configuration (DeviceConfig, ModemConfig, MqttConfig).
    Config(
        Option<DeviceConfig>,
        Option<ModemConfig>,
        Option<MqttConfig>,
    ),
    /// MiniCallHome log.
    MiniCallHomeLog(Vec<u8, 54>),
    /// LoggedAtResponse log.
    LoggedAtResponseLog(Vec<u8, 384>),
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

/// Reads packets from USB, handles commands, and sends responses.
pub struct SendPunchUsb<T: CdcAcm, M: Modem + 'static, F: Flash + 'static> {
    reader: UsbPacketReader<T>,
    send_punch_mutex: &'static Mutex<RawMutex, Option<SendPunch<M, F>>>,
    flash_mutex: &'static Mutex<RawMutex, Option<F>>,
}

impl<T: CdcAcm, M: Modem + 'static, F: Flash + 'static> SendPunchUsb<T, M, F> {
    /// Creates a new `SendPunchUsbPacketReader`.
    pub fn new(
        class: T,
        send_punch_mutex: &'static Mutex<RawMutex, Option<SendPunch<M, F>>>,
        flash_mutex: &'static Mutex<RawMutex, Option<F>>,
    ) -> Self {
        Self {
            reader: UsbPacketReader::new(class),
            send_punch_mutex,
            flash_mutex,
        }
    }

    /// Locks the flash mutex and returns a guard providing access to the flash instance.
    pub async fn lock_flash(&self) -> FlashGuard<'static, F> {
        let guard = self.flash_mutex.lock().await;
        FlashGuard::new(guard)
    }

    async fn write_response(&mut self, response: UsbResponse) -> Result<(), Error> {
        let response_bytes = postcard::to_vec::<_, 448>(&response)?;
        self.reader.write(response_bytes.as_slice()).await
    }

    /// Responds to a USB command.
    pub async fn respond(&mut self, command: UsbCommand) -> Result<(), Error> {
        match command {
            UsbCommand::Handshake => {
                info!("Handshake request");
                let magic = String::try_from("YAROC").map_err(|_| Error::StringEncodingError)?;
                self.write_response(UsbResponse::Handshake(magic, PROTOCOL_VERSION)).await?;
            }
            UsbCommand::GetConfig => {
                info!("Request to read all configs from flash");
                let mut flash = self.lock_flash().await;
                let device_config = flash.read::<DeviceConfig>().await?;
                let modem_config = flash.read::<ModemConfig>().await?;
                let mqtt_config = flash.read::<MqttConfig>().await?;
                self.write_response(UsbResponse::Config(
                    device_config,
                    modem_config,
                    mqtt_config,
                ))
                .await?;
            }
            UsbCommand::ConfigureModem(modem_config) => {
                {
                    let mut flash = self.lock_flash().await;
                    flash.write(modem_config.clone()).await?;
                }
                info!("Modem config written to flash");
                self.write_response(UsbResponse::PartialOk(155_000)).await?;
                {
                    let mut send_punch = self.send_punch_mutex.lock().await;
                    let send_punch = send_punch.as_mut().expect("SendPunch not initialized");
                    send_punch.configure_modem(modem_config).await?;
                }
                info!("Modem reconfigured");
                self.write_response(UsbResponse::Ok).await?;
            }
            UsbCommand::ConfigureMqtt(mqtt_config) => {
                {
                    let mut flash = self.lock_flash().await;
                    flash.write(mqtt_config.clone()).await?;
                }
                info!("MQTT config written to flash");
                self.write_response(UsbResponse::PartialOk(125_000)).await?;
                {
                    let mut send_punch = self.send_punch_mutex.lock().await;
                    let send_punch = send_punch.as_mut().expect("SendPunch not initialized");
                    send_punch.configure_mqtt(mqtt_config).await?;
                }
                info!("MQTT reconfigured");
                self.write_response(UsbResponse::Ok).await?;
            }
            UsbCommand::ConfigureDevice(device_config) => {
                {
                    let mut send_punch = self.send_punch_mutex.lock().await;
                    let send_punch = send_punch.as_mut().expect("SendPunch not initialized");
                    send_punch.update_device_config(device_config).await?;
                }
                // TODO: we should restart tasks to apply the settings, or restart the whole device?
                self.write_response(UsbResponse::Ok).await?;
            }
            UsbCommand::EraseFlash => {
                info!("Request to erase the flash");
                let mut flash = self.lock_flash().await;
                self.write_response(UsbResponse::PartialOk(8_000)).await?;
                flash.erase().await?;
                info!("Flash erased");
                self.write_response(UsbResponse::Ok).await?;
            }
            UsbCommand::GetMiniCallHomeLogs => {
                info!("Request to read all MiniCallHome logs");
                let mut flash = self.lock_flash().await;
                let mut iter = flash.mch_iter().await?;
                loop {
                    self.write_response(UsbResponse::PartialOk(3000)).await?;
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
                info!("All MiniCallHome logs read");
                self.write_response(UsbResponse::Ok).await?;
            }
            UsbCommand::GetLoggedAtResponseLogs => {
                info!("Request to read all LoggedAtResponse logs");
                let mut flash = self.lock_flash().await;
                let mut iter = flash.logged_at_response_iter().await?;
                loop {
                    self.write_response(UsbResponse::PartialOk(5000)).await?;
                    match iter.next().await {
                        Ok(Some(logged_response)) => {
                            if let Ok(serialized) = postcard::to_vec::<_, 384>(&logged_response) {
                                let mut vec_buffer = Vec::new();
                                if vec_buffer.extend_from_slice(serialized.as_slice()).is_ok() {
                                    self.write_response(UsbResponse::LoggedAtResponseLog(
                                        vec_buffer,
                                    ))
                                    .await?;
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            error!("Error reading AT response log from flash: {}", e);
                            break;
                        }
                    }
                }
                self.write_response(UsbResponse::Ok).await?;
            }
        }
        Ok(())
    }

    /// Continuously reads commands from USB and handles them.
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

    #[test]
    fn test_handshake_serialization() {
        let command = UsbCommand::Handshake;
        let bytes = to_vec::<_, 8>(&command).unwrap();
        let decoded: UsbCommand = from_bytes(bytes.as_slice()).unwrap();
        assert_matches!(decoded, UsbCommand::Handshake);

        let magic = String::try_from("YAROC").unwrap();
        let response = UsbResponse::Handshake(magic, PROTOCOL_VERSION);
        let bytes = to_vec::<_, 32>(&response).unwrap();
        let decoded: UsbResponse = from_bytes(bytes.as_slice()).unwrap();
        assert_eq!(
            decoded,
            UsbResponse::Handshake(String::try_from("YAROC").unwrap(), PROTOCOL_VERSION)
        );
    }

    #[test]
    fn test_get_config_serialization() {
        let command = UsbCommand::GetConfig;
        let bytes = to_vec::<_, 8>(&command).unwrap();
        let decoded: UsbCommand = from_bytes(bytes.as_slice()).unwrap();
        assert_matches!(decoded, UsbCommand::GetConfig);

        let response = UsbResponse::Config(
            Some(DeviceConfig::default()),
            Some(ModemConfig::default()),
            Some(MqttConfig::default()),
        );
        let bytes = to_vec::<_, 576>(&response).unwrap();
        let decoded: UsbResponse = from_bytes(bytes.as_slice()).unwrap();
        assert_eq!(
            decoded,
            UsbResponse::Config(
                Some(DeviceConfig::default()),
                Some(ModemConfig::default()),
                Some(MqttConfig::default()),
            )
        );
    }

    #[test]
    fn test_partial_ok_response_serialization() {
        let response_partial = UsbResponse::PartialOk(150_000);
        let bytes = to_vec::<_, 16>(&response_partial).unwrap();
        let decoded: UsbResponse = from_bytes(bytes.as_slice()).unwrap();
        assert_eq!(decoded, UsbResponse::PartialOk(150_000));

        let response_ok = UsbResponse::Ok;
        let bytes = to_vec::<_, 16>(&response_ok).unwrap();
        let decoded: UsbResponse = from_bytes(bytes.as_slice()).unwrap();
        assert_eq!(decoded, UsbResponse::Ok);
    }
}
