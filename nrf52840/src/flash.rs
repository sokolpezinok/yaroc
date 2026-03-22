use embassy_embedded_hal::flash::partition::Partition;
use embassy_sync::mutex::Mutex;
use nrf_softdevice::Flash as SdFlash;
use sequential_storage::{
    cache::NoCache,
    map::{MapConfig, MapStorage, Value},
    queue::{QueueConfig, QueueStorage},
};
pub use yaroc_common::flash::{Flash, ValueIndex};
use yaroc_common::{RawMutex, error::Error};

unsafe extern "C" {
    // These symbols are provided by the linker script (memory.x)
    unsafe static _data_flash_start: u32;
    unsafe static _data_flash_size: u32;
}

/// Flash abstraction for storing serializeable objects.
pub struct NrfFlash<'a> {
    map_storage: MapStorage<u8, Partition<'a, RawMutex, SdFlash>, NoCache>,
    _queue_storage: QueueStorage<Partition<'a, RawMutex, SdFlash>, NoCache>,
}

// nrf_softdevice::Flash is !Send because it contains a *mut (), but on nRF52840
// (single core) it is safe to move between tasks as they all run in Thread Mode.
unsafe impl Send for NrfFlash<'_> {}

const MAP_SIZE: u32 = 8 * 1024;

impl<'a> NrfFlash<'a> {
    /// Creates a new NrfFlash instance
    pub fn new(flash: &'a Mutex<RawMutex, SdFlash>) -> Self {
        let data_start = unsafe { &_data_flash_start as *const u32 as u32 };
        let data_size = unsafe { &_data_flash_size as *const u32 as u32 };

        let queue_size = data_size - MAP_SIZE;

        let map_partition = Partition::new(flash, data_start, MAP_SIZE);
        let config = MapConfig::new(0..MAP_SIZE);
        let map_storage = MapStorage::new(map_partition, config, NoCache::new());

        let queue_partition = Partition::new(flash, data_start + MAP_SIZE, queue_size);
        let queue_config = QueueConfig::new(0..queue_size);
        let queue_storage = QueueStorage::new(queue_partition, queue_config, NoCache::new());

        Self {
            map_storage,
            _queue_storage: queue_storage,
        }
    }
}

impl<'a> Flash for NrfFlash<'a> {
    /// Erases the data flash memory.
    async fn erase(&mut self) -> crate::Result<()> {
        self.map_storage.erase_all().await.map_err(|_| Error::FlashError) //TODO: wrap the error
    }

    /// Stores a value in the flash memory.
    async fn write<'b, V: Value<'b>>(&mut self, key: ValueIndex, value: V) -> crate::Result<()> {
        let key = key as u8;
        let mut buffer = [0u8; 512];
        self.map_storage
            .store_item(&mut buffer, &key, &value)
            .await
            .map_err(|_| Error::FlashError)
    }

    /// Fetches a value from the flash memory.
    async fn read<'b, V: Value<'b>>(
        &mut self,
        key: ValueIndex,
        buffer: &'b mut [u8],
    ) -> crate::Result<Option<V>> {
        let key = key as u8;
        self.map_storage.fetch_item(buffer, &key).await.map_err(|_| Error::FlashError)
    }
}
