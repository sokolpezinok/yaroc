use embassy_nrf::temp::Temp as EmbassyNrfTemp;

pub trait Temp {
    fn cpu_temperature(&mut self) -> impl core::future::Future<Output = f32>;
}

pub struct NrfTemp {
    temp: EmbassyNrfTemp<'static>,
}

impl NrfTemp {
    pub fn new(temp: EmbassyNrfTemp<'static>) -> Self {
        Self { temp }
    }
}

impl Temp for NrfTemp {
    async fn cpu_temperature(&mut self) -> f32 {
        let temp = self.temp.read().await;
        temp.to_num::<f32>()
    }
}
