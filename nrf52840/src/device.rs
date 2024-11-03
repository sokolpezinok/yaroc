use defmt::*;
use embassy_nrf::bind_interrupts;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::peripherals::{P0_17, P1_03, P1_04, TIMER0, UARTE1};
use embassy_nrf::uarte::{self, UarteRxWithIdle, UarteTx};
use embassy_time::Timer;
use crate::at_utils::split_lines;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    UARTE1 => uarte::InterruptHandler<UARTE1>;
});

const AT_BUF_SIZE: usize = 300;

pub struct Device<'a> {
    rx1: UarteRxWithIdle<'a, UARTE1, TIMER0>,
    tx1: UarteTx<'a, UARTE1>,
    blue_led: Output<'a, P1_04>,
    green_led: Output<'a, P1_03>,
    modem_pin: Output<'a, P0_17>,
}

impl Device<'_> {
    pub fn new() -> Self {
        let p = embassy_nrf::init(Default::default());
        let uart1 = uarte::Uarte::new(p.UARTE1, Irqs, p.P0_15, p.P0_16, Default::default());
        let (tx1, rx1) = uart1.split_with_idle(p.TIMER0, p.PPI_CH0, p.PPI_CH1);
        let green_led = Output::new(p.P1_03, Level::Low, OutputDrive::Standard);
        let blue_led = Output::new(p.P1_04, Level::Low, OutputDrive::Standard);
        let modem_pin = Output::new(p.P0_17, Level::Low, OutputDrive::Standard);
        Self {
            rx1,
            tx1,
            blue_led,
            green_led,
            modem_pin,
        }
    }

    async fn turn_on_bg77(&mut self) {
        self.modem_pin.set_low();
        Timer::after_millis(1000).await;
        self.modem_pin.set_high();
        Timer::after_millis(2000).await;
        self.modem_pin.set_low();
        Timer::after_millis(1000).await;
    }

    pub async fn read_uart1(&mut self) {
        self.green_led.set_high();
        let mut buf = [0; 5];
        buf.copy_from_slice(b"ATI\r\n");
        self.tx1.write(&buf).await.unwrap();

        let mut buf = [0; AT_BUF_SIZE];
        let r = self.rx1.read_until_idle(&mut buf).await;
        if r.is_err() {
            return;
        }

        let len = r.unwrap();
        let lines = split_lines(&buf[..len]);
        for line in lines {
            info!("Read {}", line);
        }
        Timer::after_millis(500).await;
        self.green_led.set_low();
    }
}
