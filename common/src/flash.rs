use core::ops::{Deref, DerefMut};
use embassy_sync::mutex::MutexGuard;
use sequential_storage::map::Value;

use crate::RawMutex;
use crate::at::response::{LoggedAtResponse, PendingLoggedAtResponse};
use crate::proto::MiniCallHome as MiniCallHomeProto;
use crate::status::MiniCallHome;

/// A guard providing mutable access to the flash instance while holding the mutex lock.
pub struct FlashGuard<'a, F: Flash + 'static> {
    guard: MutexGuard<'a, RawMutex, Option<F>>,
}

impl<'a, F: Flash + 'static> FlashGuard<'a, F> {
    pub fn new(guard: MutexGuard<'a, RawMutex, Option<F>>) -> Self {
        Self { guard }
    }
}

impl<'a, F: Flash + 'static> Deref for FlashGuard<'a, F> {
    type Target = F;

    fn deref(&self) -> &Self::Target {
        self.guard.as_ref().expect("Flash not initialized")
    }
}

impl<'a, F: Flash + 'static> DerefMut for FlashGuard<'a, F> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard.as_mut().expect("Flash not initialized")
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueIndex {
    DeviceConfig = 0,
    ModemConfig = 1,
    MqttConfig = 2,
}

pub trait FlashValue: for<'a> Value<'a> {
    const VALUE_INDEX: ValueIndex;
}

pub trait Flash {
    /// Erases the data flash memory.
    fn erase(&mut self) -> impl Future<Output = crate::Result<()>>;

    /// Stores a value in the flash memory.
    fn write<V: FlashValue>(&mut self, value: V) -> impl Future<Output = crate::Result<()>>;

    /// Stores a MiniCallHome in flash (serialized as a proto).
    fn log_minicallhome(&mut self, mch: MiniCallHome) -> impl Future<Output = crate::Result<()>>;

    /// Stores a PendingLoggedAtResponse in flash.
    fn log_at_response(
        &mut self,
        response: PendingLoggedAtResponse,
    ) -> impl Future<Output = crate::Result<()>>;

    /// Fetches a value from the flash memory.
    fn read<V: FlashValue>(&mut self) -> impl Future<Output = crate::Result<Option<V>>>;

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
