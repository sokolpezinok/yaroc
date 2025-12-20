use femtopb::Message;
use nrf_softdevice::Flash as NrfFlash;
use sequential_storage::{
    cache::NoCache,
    map::{MapConfig, MapStorage, SerializationError, Value},
};
use yaroc_common::error::Error;

use crate::device::DeviceConfig;

unsafe extern "C" {
    // These symbols are provided by the linker script (memory.x)
    unsafe static _data_flash_start: u32;
    unsafe static _data_flash_size: u32;
}

/// A wrapper around the nrf_softdevice::Flash
pub struct Flash {
    map_storage: MapStorage<u8, nrf_softdevice::Flash, NoCache>,
}

impl<'a> Value<'a> for DeviceConfig<'a> {
    fn serialize_into(&self, mut buffer: &mut [u8]) -> Result<usize, SerializationError> {
        self.encode(&mut buffer).map_err(|_| SerializationError::BufferTooSmall)?;
        let len = self.encoded_len();
        Ok(len)
    }

    fn deserialize_from(buffer: &'a [u8]) -> Result<(Self, usize), SerializationError> {
        Self::decode(buffer).map_err(|_| SerializationError::InvalidData).map(|d| {
            let len = d.encoded_len();
            (d, len)
        })
    }
}

#[repr(u8)]
pub enum ValueIndex {
    DeviceConfig = 0,
}

impl Flash {
    /// Creates a new Flash instance
    pub fn new(flash: NrfFlash) -> Self {
        let data_start = unsafe { &_data_flash_start as *const u32 as u32 };
        let data_end = data_start + unsafe { &_data_flash_size as *const u32 as u32 };
        let config = MapConfig::new(data_start..data_end);
        let map_storage = MapStorage::new(flash, config, NoCache::new());

        Self { map_storage }
    }

    /// Erases the data flash memory.
    pub async fn erase(&mut self) -> crate::Result<()> {
        self.map_storage.erase_all().await.map_err(|_| Error::FlashError)
    }

    /// Stores a value in the flash memory.
    pub async fn write<'a, V: Value<'a>>(
        &mut self,
        key: ValueIndex,
        value: V,
        buffer: &'a mut [u8],
    ) -> crate::Result<()> {
        let key = key as u8;

        self.map_storage
            .store_item(buffer, &key, &value)
            .await
            .map_err(|_| Error::FlashError)
    }

    /// Fetches a value from the flash memory.
    pub async fn read<'a, V: Value<'a>>(
        &mut self,
        key: ValueIndex,
        buffer: &'a mut [u8],
    ) -> crate::Result<Option<V>> {
        let key = key as u8;
        self.map_storage.fetch_item(buffer, &key).await.map_err(|_| Error::FlashError)
    }
}
