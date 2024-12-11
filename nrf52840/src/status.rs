use embassy_nrf::temp::Temp as EmbassyNrfTemp;

pub trait Temp {
    fn cpu_temperature(&mut self) -> impl core::future::Future<Output = i8>;
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
    async fn cpu_temperature(&mut self) -> i8 {
        let temp = self.temp.read().await;
        temp.to_num::<i8>() // TODO: Has two fractional bits
    }
}
