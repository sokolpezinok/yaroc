use core::str::FromStr;

#[cfg(feature = "defmt")]
use defmt::{debug, error, info, warn};
use embassy_sync::channel::Sender;
use embassy_time::{Duration, Instant, Timer};
use heapless::{String, format};
#[cfg(not(feature = "defmt"))]
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};

use crate::RawMutex;
use crate::at::response::CommandResponse;
use crate::bg77::hw::ModemHw;
use crate::error::Error;
use crate::send_punch::SendPunchCommand;

#[cfg(feature = "nrf")]
use embassy_nrf::gpio::Output;

/// Trait for controlling the modem power pin.
pub trait ModemPin {
    /// Sets the pin output to high.
    fn set_high(&mut self);
    /// Sets the pin output to low.
    fn set_low(&mut self);
}

pub struct FakePin {}

impl ModemPin for FakePin {
    fn set_high(&mut self) {}
    fn set_low(&mut self) {}
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
#[derive(Default, Clone, Copy, Serialize, Deserialize)]
pub enum RAT {
    Ltem,  // LTE-M
    NbIot, // NB-IoT
    #[default]
    LtemNbIot, // Both
}

#[derive(Default, Clone, Copy, Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize)]
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

/// Manages the BG77 modem configuration and connection state.
pub struct ModemManager {
    config: ModemConfig,
}

impl ModemManager {
    /// Creates a new ModemManager with the given configuration.
    pub fn new(config: ModemConfig) -> Self {
        Self { config }
    }

    /// Handles Unsolicited Result Codes (URC) from the modem.
    ///
    /// Returns true if the URC indicates a significant event that should trigger
    /// further action, false otherwise.
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

    /// Powers on the modem hardware.
    ///
    /// Tries to communicate with the modem. If it doesn't respond, it toggles the power pin
    /// to reset/turn on the modem.
    pub async fn turn_on<M: ModemHw, P: ModemPin>(
        &self,
        bg77: &mut M,
        modem_pin: &mut P,
    ) -> Result<(), Error> {
        if bg77.call_at("E0", None).await.is_err() {
            modem_pin.set_low();
            Timer::after_secs(1).await;
            modem_pin.set_high();
            Timer::after_secs(2).await;
            modem_pin.set_low();
            let res = bg77.read("", Duration::from_secs(1)).await?;
            debug!("Modem response: {}", res);
            bg77.long_call_at("+CFUN=1,0", Duration::from_secs(15)).await?;
            let res = bg77.read("", Duration::from_secs(5)).await?;
            debug!("Modem response: {}", res);
        }
        Ok(())
    }

    /// Updates the modem configuration.
    pub fn update_config(&mut self, modem_config: ModemConfig) {
        self.config = modem_config;
    }

    /// Configures the modem with the current settings (APN, RAT, Bands).
    pub async fn configure<M: ModemHw>(&self, bg77: &mut M) -> Result<(), Error> {
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

    /// Registers the modem to the network.
    ///
    /// This function first checks if any MQTT messages have been published recently.
    /// If no messages have been sent for a prolonged period (determined by `packet_timeout` and `cgatt_cnt`),
    /// it attempts to reattach to the network by deactivating and reactivating the GPRS context.
    /// Otherwise, it checks the current network registration status and registers if not already registered.
    pub async fn network_registration<M: ModemHw>(
        &self,
        bg77: &mut M,
        force_reattach: bool,
    ) -> crate::Result<()> {
        if force_reattach {
            warn!("Will reattach to network because of no messages being sent for a long time");
            bg77.call_at("E0", None).await?;
            let _ = bg77.long_call_at("+CGATT=0", ACTIVATION_TIMEOUT).await;
            Timer::after_secs(2).await;
            let _ = bg77.long_call_at("+CGACT=0,1", ACTIVATION_TIMEOUT).await;
        } else {
            let state = bg77.call_at("+CGATT?", None).await?.parse1::<u8>([0], None)?;
            if state == 1 {
                info!("Already registered to network");
                return Ok(());
            }
        }

        bg77.long_call_at("+CGATT=1", ACTIVATION_TIMEOUT).await?;
        // CGATT=1 needs additional time and reading from modem
        Timer::after_secs(1).await;
        // TODO: this is the only ModemHw::read() in the code base, can it be removed?
        let _response = bg77.read("+CGATT", Duration::from_secs(1)).await;
        #[cfg(feature = "defmt")]
        if let Ok(response) = _response
            && !response.lines().is_empty()
        {
            debug!("Read {=[?]} after CGATT=1", response.lines());
        }
        // TODO: should we do something with the result?
        let (_, _) = bg77.call_at("+CGACT?", None).await?.parse2::<u8, u8>([0, 1], Some(1))?;

        Ok(())
    }
}

#[cfg(feature = "std")]
#[cfg(test)]
mod test {
    use crate::at::fake_modem::FakeModem;

    use super::*;
    use embassy_futures::block_on;

    #[test]
    fn test_configure_modem() {
        let mut config = ModemConfig::default();
        config.apn = String::from_str("test-apn").unwrap();
        config.bands.set_ltem_bands(&[3]);
        let modem_manager = ModemManager::new(config);

        let mut bg77 = FakeModem::new(&[
            ("AT+CGDCONT=1,\"IP\",\"test-apn\"", ""),
            ("AT+CEREG=2", ""),
            ("AT+CGATT=1", ""),
            ("AT+QCFG=\"nwscanseq\",00", ""),
            ("AT+QCFG=\"iotopmode\",2,1", ""),
            ("AT+QCFG=\"band\",0,4,80000", ""),
        ]);
        assert!(block_on(modem_manager.configure(&mut bg77)).is_ok());
        assert!(bg77.all_done());
    }
}
