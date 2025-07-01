use embassy_sync::watch::Watch;

use crate::RawMutex;

pub trait Temp {
    fn cpu_temperature(&mut self) -> impl core::future::Future<Output = crate::Result<f32>>;
}

#[cfg(feature = "nrf")]
pub struct NrfTemp {
    temp: embassy_nrf::temp::Temp<'static>,
}

#[cfg(feature = "nrf")]
impl NrfTemp {
    pub fn new(temp: embassy_nrf::temp::Temp<'static>) -> Self {
        Self { temp }
    }
}

#[cfg(feature = "nrf")]
impl Temp for NrfTemp {
    async fn cpu_temperature(&mut self) -> crate::Result<f32> {
        let temp = self.temp.read().await;
        Ok(temp.to_num::<f32>())
    }
}

pub static TEMPERATURE: Watch<RawMutex, f32, 1> = Watch::new();

#[derive(Clone, Copy)]
pub struct BatteryInfo {
    pub mv: u16,
    pub percents: u8,
}
pub static BATTERY: Watch<RawMutex, BatteryInfo, 1> = Watch::new();
