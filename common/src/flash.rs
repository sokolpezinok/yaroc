use sequential_storage::map::Value;

use crate::at::response::LoggedAtResponse;
use crate::proto::MiniCallHome as MiniCallHomeProto;
use crate::status::MiniCallHome;

#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum ValueIndex {
    DeviceConfig = 0,
    ModemConfig = 1,
    MqttConfig = 2,
}

pub trait Flash {
    /// Erases the data flash memory.
    fn erase(&mut self) -> impl Future<Output = crate::Result<()>>;

    /// Stores a value in the flash memory.
    fn write<'a, V: Value<'a>>(
        &mut self,
        key: ValueIndex,
        value: V,
    ) -> impl Future<Output = crate::Result<()>>;

    /// Stores a MiniCallHome in flash (serialized as a proto).
    fn log_minicallhome(&mut self, mch: MiniCallHome) -> impl Future<Output = crate::Result<()>>;

    /// Stores a LoggedAtResponse in flash.
    fn log_at_response(
        &mut self,
        response: LoggedAtResponse,
    ) -> impl Future<Output = crate::Result<()>>;

    /// Fetches a value from the flash memory.
    fn read<V: for<'a> Value<'a>>(
        &mut self,
        key: ValueIndex,
    ) -> impl Future<Output = crate::Result<Option<V>>>;

    type MchIter<'a>: MchIterator
    where
        Self: 'a;

    /// Returns an iterator over the stored MiniCallHome messages.
    fn mch_iter(&mut self) -> impl Future<Output = crate::Result<Self::MchIter<'_>>>;

    type LoggedAtResponseIter<'a>: LoggedAtResponseIterator
    where
        Self: 'a;

    /// Returns an iterator over the stored LoggedAtResponse messages.
    fn logged_at_response_iter(
        &mut self,
    ) -> impl Future<Output = crate::Result<Self::LoggedAtResponseIter<'_>>>;
}

pub trait MchIterator {
    fn next<'b>(&'b mut self)
    -> impl Future<Output = crate::Result<Option<MiniCallHomeProto<'b>>>>;
}

pub trait LoggedAtResponseIterator {
    fn next(&mut self) -> impl Future<Output = crate::Result<Option<LoggedAtResponse>>>;
}
