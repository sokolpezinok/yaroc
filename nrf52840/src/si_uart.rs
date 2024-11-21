use embassy_nrf::{peripherals::UARTE0, uarte::UarteRx};

// SportIdent UART. Reads chunks of 20 bytes.
pub struct SiUart {
    rx: UarteRx<'static, UARTE0>,
}

impl SiUart {
    pub fn new(rx: UarteRx<'static, UARTE0>) -> Self {
        Self { rx }
    }
}
