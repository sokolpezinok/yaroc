use embedded_storage_async::nor_flash::ReadNorFlash;
use nrf_softdevice::Flash as NrfFlash;
use yaroc_common::error::Error;

unsafe extern "C" {
    // These symbols are provided by the linker script (memory.x)
    unsafe static _data_flash_start: u32;
    unsafe static _data_flash_size: u32;
}

/// A wrapper around the nrf_softdevice::Flash
pub struct Flash {
    inner: NrfFlash,
    data_start: u32,
}

impl Flash {
    /// Creates a new Flash instance
    pub fn new(flash: NrfFlash) -> Self {
        let data_start = unsafe { &_data_flash_start as *const u32 as u32 };
        Self {
            inner: flash,
            data_start,
        }
    }

    /// Reads data from the flash memory.
    pub async fn read(&mut self, offset: u32) -> crate::Result<()> {
        let mut b = [0u8; 512];
        self.inner
            .read(self.data_start + offset, &mut b)
            .await
            .map_err(|_| Error::FlashError)
    }
}
