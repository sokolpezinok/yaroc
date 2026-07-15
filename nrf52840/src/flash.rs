use embassy_embedded_hal::flash::partition::Partition;
use embassy_sync::mutex::Mutex;
use femtopb::Message as _;
use nrf_softdevice::Flash as SdFlash;
use sequential_storage::{
    cache::NoCache,
    map::{MapConfig, MapStorage, Value},
    queue::{QueueConfig, QueueStorage},
};

use yaroc_common::flash::{Flash, ValueIndex};
use yaroc_common::{RawMutex, error::Error, status::MiniCallHome};

unsafe extern "C" {
    // These symbols are provided by the linker script (memory.x)
    unsafe static _data_flash_start: u32;
    unsafe static _data_flash_size: u32;
}

/// Flash abstraction for storing serializeable objects.
pub struct NrfFlash<'a> {
    map_storage: MapStorage<u8, Partition<'a, RawMutex, SdFlash>, NoCache>,
    mch_storage: QueueStorage<Partition<'a, RawMutex, SdFlash>, NoCache>,
    queue_storage: QueueStorage<Partition<'a, RawMutex, SdFlash>, NoCache>,
}

// nrf_softdevice::Flash is !Send because it contains a *mut (), but on nRF52840
// (single core) it is safe to move between tasks as they all run in Thread Mode.
unsafe impl Send for NrfFlash<'_> {}

const MAP_SIZE: u32 = 8 * 1024;
const MCH_SIZE: u32 = 24 * 1024;

impl<'a> NrfFlash<'a> {
    /// Creates a new NrfFlash instance
    pub fn new(flash: &'a Mutex<RawMutex, SdFlash>) -> Self {
        let data_start = unsafe { &_data_flash_start as *const u32 as u32 };
        let data_size = unsafe { &_data_flash_size as *const u32 as u32 };

        let queue_size = data_size - MAP_SIZE - MCH_SIZE;

        let map_partition = Partition::new(flash, data_start, MAP_SIZE);
        let config = MapConfig::new(0..MAP_SIZE);
        let map_storage = MapStorage::new(map_partition, config, NoCache::new());

        let mch_partition = Partition::new(flash, data_start + MAP_SIZE, MCH_SIZE);
        let mch_config = QueueConfig::new(0..MCH_SIZE);
        let mch_storage = QueueStorage::new(mch_partition, mch_config, NoCache::new());

        let queue_partition = Partition::new(flash, data_start + MAP_SIZE + MCH_SIZE, queue_size);
        let queue_config = QueueConfig::new(0..queue_size);
        let queue_storage = QueueStorage::new(queue_partition, queue_config, NoCache::new());

        Self {
            map_storage,
            mch_storage,
            queue_storage,
        }
    }
}

impl<'a> Flash for NrfFlash<'a> {
    /// Erases the data flash memory.
    async fn erase(&mut self) -> crate::Result<()> {
        // TODO: wrap and propagate the error
        self.map_storage.erase_all().await.map_err(|_| Error::FlashError)?;
        self.mch_storage.erase_all().await.map_err(|_| Error::FlashError)?;
        self.queue_storage.erase_all().await.map_err(|_| Error::FlashError)
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

    /// Stores a MiniCallHome in flash (serialized as a proto).
    async fn log_minicallhome(&mut self, mch: MiniCallHome) -> crate::Result<()> {
        let status = mch.to_proto();
        let mch_proto = match status.msg {
            Some(yaroc_common::proto::status::Msg::MiniCallHome(p)) => p,
            _ => return Err(Error::ValueError),
        };

        let mut buffer = [0u8; 256];
        mch_proto
            .encode(&mut buffer.as_mut_slice())
            .map_err(|_| Error::BufferTooSmallError)?;
        let len = mch_proto.encoded_len();
        self.mch_storage.push(&buffer[..len], true).await.map_err(|_| Error::FlashError)
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
