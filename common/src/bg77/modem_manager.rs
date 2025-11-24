use core::str::FromStr;

#[cfg(feature = "defmt")]
use defmt::error;
use embassy_sync::channel::Sender;
use embassy_time::{Duration, Instant};
use heapless::{String, format};
#[cfg(not(feature = "defmt"))]
use log::error;

use crate::RawMutex;
use crate::at::response::CommandResponse;
use crate::bg77::hw::ModemHw;
use crate::error::Error;
use crate::send_punch::SendPunchCommand;

#[cfg(feature = "nrf")]
use embassy_nrf::gpio::Output;

pub trait ModemPin {
    fn set_high(&mut self);
    fn set_low(&mut self);
}

#[cfg(feature = "nrf")]
impl ModemPin for Output<'static> {
    fn set_high(&mut self) {
        self.set_high();
    }

    fn set_low(&mut self) {
        self.set_low();
    }
}

/// Timeout for network activation.
pub static ACTIVATION_TIMEOUT: Duration = Duration::from_secs(150);

/// Radio Access Technology
pub enum RAT {
    Ltem,      // LTE-M
    NbIot,     // NB-IoT
    LtemNbIot, // Both
}

#[derive(Default, Clone, Copy)]
pub struct LteBands {
    /// LTE-M bands bitmask. Bit `n` corresponds to band `n+1`.
    pub ltem: u128,
    /// NB-IoT bands bitmask. Bit `n` corresponds to band `n+1`.
    pub nbiot: u128,
}

impl LteBands {
    /// Sets the LTE-M bands from a slice of band numbers.
    ///
    /// This will overwrite any previously set LTE-M bands.
    /// Bands should be given as numbers, e.g., 20 for B20.
    /// Invalid band numbers (0 or > 128) are ignored.
    pub fn set_ltem_bands(&mut self, bands: &[u32]) {
        self.ltem = 0;
        for &band in bands {
            if band > 0 && band <= 128 {
                self.ltem |= 1_u128 << (band - 1);
            }
        }
    }

    /// Sets the NB-IoT bands from a slice of band numbers.
    ///
    /// This will overwrite any previously set NB-IoT bands.
    /// Bands should be given as numbers, e.g., 20 for B20.
    /// Invalid band numbers (0 or > 128) are ignored.
    pub fn set_nbiot_bands(&mut self, bands: &[u32]) {
        self.nbiot = 0;
        for &band in bands {
            if band > 0 && band <= 128 {
                self.nbiot |= 1_u128 << (band - 1);
            }
        }
    }
}

pub struct ModemConfig {
    /// Access point name (APN)
    pub apn: String<30>,
    /// Radio access technology (RAT)
    pub rat: RAT,
    /// LTE bands
    pub bands: LteBands,
}

impl Default for ModemConfig {
    /// Creates a default modem configuration.
    fn default() -> Self {
        let mut bands = LteBands::default();
        // Default bands are B20 for both LTE-M and NB-IoT
        bands.set_ltem_bands(&[20]);
        bands.set_nbiot_bands(&[20]);
        Self {
            apn: String::from_str("internet.iot").unwrap(),
            rat: RAT::LtemNbIot,
            bands,
        }
    }
}

pub struct ModemManager {
    config: ModemConfig,
}

impl ModemManager {
    pub fn new(config: ModemConfig) -> Self {
        Self { config }
    }

    pub fn urc_handler(
        response: &'_ CommandResponse,
        command_sender: Sender<'static, RawMutex, SendPunchCommand, 10>,
    ) -> bool {
        match response.command() {
            "QIURC" => {
                let message = SendPunchCommand::NetworkConnect(Instant::now());
                if command_sender.try_send(message).is_err() {
                    error!("Channel full when sending network connect command");
                }
                true
            }
            "CEREG" => response.values().len() == 4,
            _ => false,
        }
    }

    pub async fn turn_on<M: ModemHw, P: ModemPin>(
        &self,
        bg77: &mut M,
        modem_pin: &mut P,
    ) -> Result<(), Error> {
        if bg77.call_at("", None).await.is_err() {
            modem_pin.set_low();
            embassy_time::Timer::after_secs(1).await;
            modem_pin.set_high();
            embassy_time::Timer::after_secs(2).await;
            modem_pin.set_low();
            // TODO: fix command
            let _res = bg77.read("", Duration::from_secs(5)).await?;
            #[cfg(feature = "defmt")]
            defmt::info!("Modem response: {=[?]}", _res.lines());
            bg77.long_call_at("+CFUN=1,0", Duration::from_secs(15)).await?;
            let _res = bg77.read("", Duration::from_secs(5)).await?;
            #[cfg(feature = "defmt")]
            defmt::info!("Modem response: {=[?]}", _res.lines());
        }
        Ok(())
    }

    pub async fn configure<M: ModemHw>(&self, bg77: &mut M) -> Result<(), Error> {
        bg77.call_at("E0", None).await?;
        let cmd = format!(100; "+CGDCONT=1,\"IP\",\"{}\"", self.config.apn)?;
        let _ = bg77.call_at(&cmd, None).await;
        bg77.call_at("+CEREG=2", None).await?;
        let _ = bg77.long_call_at("+CGATT=1", ACTIVATION_TIMEOUT).await;

        let (nwscanseq, iotopmode) = match self.config.rat {
            RAT::Ltem => ("02", 0),
            RAT::NbIot => ("03", 1),
            RAT::LtemNbIot => ("00", 2),
        };
        let cmd = format!(50; "+QCFG=\"nwscanseq\",{}", nwscanseq)?;
        bg77.call_at(&cmd, None).await?;
        let cmd = format!(50; "+QCFG=\"iotopmode\",{},1", iotopmode)?;
        bg77.call_at(&cmd, None).await?;
        let cmd = format!(100; "+QCFG=\"band\",0,{:x},{:x}", self.config.bands.ltem, self.config.bands.nbiot)?;
        bg77.call_at(&cmd, None).await?;
        Ok(())
    }
}
