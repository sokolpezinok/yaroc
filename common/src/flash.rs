use sequential_storage::map::Value;

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
        buffer: &'a mut [u8],
    ) -> impl Future<Output = crate::Result<()>>;

    /// Fetches a value from the flash memory.
    fn read<'a, V: Value<'a>>(
        &mut self,
        key: ValueIndex,
        buffer: &'a mut [u8],
    ) -> impl Future<Output = crate::Result<Option<V>>>;
}
