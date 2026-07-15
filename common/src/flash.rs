use sequential_storage::map::Value;

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

    /// Fetches a value from the flash memory.
    fn read<'b, V: Value<'b>>(
        &mut self,
        key: ValueIndex,
        buffer: &'b mut [u8],
    ) -> impl Future<Output = crate::Result<Option<V>>>;
}
