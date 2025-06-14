use embedded_storage_async::nor_flash::ReadNorFlash;
use nrf_softdevice::Flash as NrfFlash;
use yaroc_common::error::Error;

pub struct Flash {
    inner: NrfFlash,
}

impl Flash {
    pub fn new(flash: NrfFlash) -> Self {
        Self { inner: flash }
    }

    pub async fn read(&mut self) -> crate::Result<()> {
        //TODO: deserialize what's been read
        let mut b = [0u8; 512];
        const ADDR: u32 = 0x80000;
        self.inner.read(ADDR, &mut b).await.map_err(|_| Error::FlashError)
    }
}
