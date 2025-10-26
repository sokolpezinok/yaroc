use femtopb::Message;
use nrf_softdevice::Flash as NrfFlash;
use sequential_storage::{
    cache::NoCache,
    map::{SerializationError, Value, fetch_item},
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
    inner: NrfFlash,
    data_start: u32,
    data_end: u32,
}

impl<'a> Value<'a> for DeviceConfig<'a> {
    fn serialize_into(&self, mut buffer: &mut [u8]) -> Result<usize, SerializationError> {
        self.encode(&mut buffer).map_err(|_| SerializationError::BufferTooSmall)?;
        let len = self.encoded_len();
        Ok(len)
    }

    fn deserialize_from(buffer: &'a [u8]) -> Result<Self, SerializationError> {
        Self::decode(buffer).map_err(move |_| SerializationError::InvalidData)
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
        let data_end = data_start + 64 * 1024; // TODO: use _data_flash_size
        Self {
            inner: flash,
            data_start,
            data_end,
        }
    }

    /// Reads data from the flash memory.
    pub async fn read<'a, V: Value<'a>>(
        &mut self,
        key: ValueIndex,
        buffer: &'a mut [u8],
    ) -> crate::Result<Option<V>> {
        let range = self.data_start..self.data_end;
        let key = key as u8;

        fetch_item::<u8, V, _>(&mut self.inner, range, &mut NoCache::new(), buffer, &key)
            .await
            .map_err(|_| Error::FlashError)
    }
}
