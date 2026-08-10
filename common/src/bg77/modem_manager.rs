use core::marker::PhantomData;
use core::str::FromStr;

#[cfg(feature = "defmt")]
use defmt::{debug, error, info, warn};
use embassy_sync::channel::Sender;
use embassy_time::{Duration, Timer};
use heapless::{String, format};
#[cfg(not(feature = "defmt"))]
use log::{error, info, warn};
use sequential_storage::map::PostcardValue;
use serde::{Deserialize, Serialize};

use crate::RawMutex;
use crate::at::AtError;
use crate::at::response::{AT_RESPONSE_SIZE, CommandResponse, FromModem};
use crate::at::uart::AtUartTrait;
use crate::bg77::connection::ConnectionEvent;
use crate::flash::{FlashValue, ValueIndex};
use crate::send_punch::SendPunchCommand;

pub use crate::bg77::modem::{FakePin, ModemPin};

/// Network Registration Error (AT+CGATT, AT+CGACT)
#[derive(Debug, thiserror::Error, Clone, Copy, Eq, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum RegistrationError {
    #[error("PDP context activation failed")]
    PdpContextFailed,
    #[error("Phone failure")]
    PhoneFailure,
    #[error("No connection to phone")]
    NoConnectionToPhone,
    #[error("Phone-adaptor link reserved")]
    PhoneAdaptorLinkReserved,
    #[error("Operation not allowed")]
    OperationNotAllowed,
    #[error("Operation not supported")]
    OperationNotSupported,
    #[error("PH-SIM PIN required")]
    PhSimPinRequired,
    #[error("PH-FSIM PIN required")]
    PhFsimPinRequired,
    #[error("PH-FSIM PUK required")]
    PhFsimPukRequired,
    #[error("(U)SIM not inserted")]
    SimNotInserted,
    #[error("(U)SIM PIN required")]
    SimPinRequired,
    #[error("(U)SIM PUK required")]
    SimPukRequired,
    #[error("(U)SIM failure")]
    SimFailure,
    #[error("(U)SIM busy")]
    SimBusy,
    #[error("(U)SIM wrong")]
    SimWrong,
    #[error("Incorrect password")]
    IncorrectPassword,
    #[error("(U)SIM PIN2 required")]
    SimPin2Required,
    #[error("(U)SIM PUK2 required")]
    SimPuk2Required,
    #[error("Memory full")]
    MemoryFull,
    #[error("Invalid index")]
    InvalidIndex,
    #[error("Not found")]
    NotFound,
    #[error("Memory failure")]
    MemoryFailure,
    #[error("Text string too long")]
    TextStringTooLong,
    #[error("Invalid characters in text string")]
    InvalidCharactersInTextString,
    #[error("Dial string too long")]
    DialStringTooLong,
    #[error("Invalid characters in dial string")]
    InvalidCharactersInDialString,
    #[error("No network service")]
    NoNetworkService,
    #[error("Network timeout")]
    NetworkTimeout,
    #[error("Network not allowed - emergency calls only")]
    NetworkNotAllowedEmergencyOnly,
    #[error("Network personalization PIN required")]
    NetworkPersonalizationPinRequired,
    #[error("Network personalization PUK required")]
    NetworkPersonalizationPukRequired,
    #[error("Network subset personalization PIN required")]
    NetworkSubsetPersonalizationPinRequired,
    #[error("Network subset personalization PUK required")]
    NetworkSubsetPersonalizationPukRequired,
    #[error("Service provider personalization PIN required")]
    ServiceProviderPersonalizationPinRequired,
    #[error("Service provider personalization PUK required")]
    ServiceProviderPersonalizationPukRequired,
    #[error("Corporate personalization PIN required")]
    CorporatePersonalizationPinRequired,
    #[error("Corporate personalization PUK required")]
    CorporatePersonalizationPukRequired,
    #[error("AT command error")]
    AtErrorResponse,
    #[error("Timeout error")]
    TimeoutError,
    #[error("Unknown registration error ({0})")]
    Unknown(u16),
}

impl RegistrationError {
    pub fn from_error(err: AtError) -> Self {
        match err {
            AtError::CmeError(code) => match code {
                0 => Self::PhoneFailure,
                1 => Self::NoConnectionToPhone,
                2 => Self::PhoneAdaptorLinkReserved,
                3 => Self::OperationNotAllowed,
                4 => Self::OperationNotSupported,
                5 => Self::PhSimPinRequired,
                6 => Self::PhFsimPinRequired,
                7 => Self::PhFsimPukRequired,
                10 => Self::SimNotInserted,
                11 => Self::SimPinRequired,
                12 => Self::SimPukRequired,
                13 => Self::SimFailure,
                14 => Self::SimBusy,
                15 => Self::SimWrong,
                16 => Self::IncorrectPassword,
                17 => Self::SimPin2Required,
                18 => Self::SimPuk2Required,
                20 => Self::MemoryFull,
                21 => Self::InvalidIndex,
                22 => Self::NotFound,
                23 => Self::MemoryFailure,
                24 => Self::TextStringTooLong,
                25 => Self::InvalidCharactersInTextString,
                26 => Self::DialStringTooLong,
                27 => Self::InvalidCharactersInDialString,
                30 => Self::NoNetworkService,
                31 => Self::NetworkTimeout,
                32 => Self::NetworkNotAllowedEmergencyOnly,
                40 => Self::NetworkPersonalizationPinRequired,
                41 => Self::NetworkPersonalizationPukRequired,
                42 => Self::NetworkSubsetPersonalizationPinRequired,
                43 => Self::NetworkSubsetPersonalizationPukRequired,
                44 => Self::ServiceProviderPersonalizationPinRequired,
                45 => Self::ServiceProviderPersonalizationPukRequired,
                46 => Self::CorporatePersonalizationPinRequired,
                47 => Self::CorporatePersonalizationPukRequired,
                code => Self::Unknown(code),
            },
            AtError::AtErrorResponse => Self::AtErrorResponse,
            AtError::TimeoutError => Self::TimeoutError,
            _ => Self::Unknown(999),
        }
    }
}

/// Timeout for network activation.
pub static ACTIVATION_TIMEOUT: Duration = Duration::from_secs(150);

/// Radio Access Technology
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RAT {
    Ltem,  // LTE-M
    NbIot, // NB-IoT
    #[default]
    LtemNbIot, // Both
}

#[derive(Default, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModemConfig {
    /// Access point name (APN)
    pub apn: String<30>,
    /// Radio access technology (RAT)
    pub rat: RAT,
    /// LTE bands
    pub bands: LteBands,
}

impl PostcardValue<'_> for ModemConfig {}

impl FlashValue for ModemConfig {
    const VALUE_INDEX: ValueIndex = ValueIndex::ModemConfig;
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
pub struct ModemManager<M> {
    config: ModemConfig,
    _phantom: PhantomData<M>,
}

impl<M: AtUartTrait> ModemManager<M> {
    /// Creates a new ModemManager with the given configuration.
    pub fn new(config: ModemConfig) -> Self {
        Self {
            config,
            _phantom: PhantomData,
        }
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
                let message =
                    SendPunchCommand::ConnectionSupervisorEvent(ConnectionEvent::PdpDeactivate);
                if command_sender.try_send(message).is_err() {
                    error!("Channel full when sending PDP deactivation event");
                }
                true
            }
            "CEREG" => response.values().len() == 4,
            _ => false,
        }
    }

    /// Updates the modem configuration.
    pub fn update_config(&mut self, modem_config: ModemConfig) {
        self.config = modem_config;
    }

    /// Configures the modem with the current settings (APN, RAT, Bands).
    ///
    /// Returns the current firmware version.
    pub async fn configure(&self, bg77: &mut M) -> crate::Result<String<AT_RESPONSE_SIZE>> {
        let firmware = bg77.call_at("+QGMR", None).await?.lines().first().and_then(|x| {
            if let FromModem::Line(line) = x {
                Some(line.clone())
            } else {
                None
            }
        });
        let cmd = format!(100; "+CGDCONT=1,\"IP\",\"{}\"", self.config.apn)?;
        bg77.call_at(&cmd, None).await?;
        bg77.call_at("+CEREG=2", None).await?;

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

        let _ = bg77.long_call_at("+CGATT=1", ACTIVATION_TIMEOUT + Duration::from_secs(1)).await;
        firmware.ok_or(AtError::ModemError.into())
    }

    /// Checks if the network is attached and PDP activated
    pub async fn is_registered(&self, bg77: &mut M) -> crate::Result<bool> {
        let gatt = bg77.call_at("+CGATT?", None).await?.parse1::<u8>([0])?;
        let (_, stat) = bg77.call_at("+CGACT?", None).await?.parse2::<u8, u8>([0, 1], Some(1))?;
        Ok(gatt == 1 && stat == 1)
    }

    /// Registers the modem to the network.
    ///
    /// This function first checks if any MQTT messages have been published recently.
    /// If no messages have been sent for a prolonged period (determined by `packet_timeout` and `cgatt_cnt`),
    /// it attempts to reattach to the network by deactivating and reactivating the GPRS context.
    /// Otherwise, it checks the current network registration status and registers if not already registered.
    pub async fn network_registration(
        &self,
        bg77: &mut M,
        force_reattach: bool,
    ) -> crate::Result<()> {
        let att_state = if force_reattach {
            warn!("Will deattach from network because of no messages being sent for a long time");
            bg77.call_at("E0", None).await?;
            let _ = bg77.long_call_at("+CGATT=0", ACTIVATION_TIMEOUT).await;
            Timer::after_secs(2).await;
            let _ = bg77.long_call_at("+CGACT=0,1", ACTIVATION_TIMEOUT).await;
            0
        } else {
            bg77.call_at("+CGATT?", None).await?.parse1::<u8>([0])?
        };

        if att_state != 1 {
            info!("Will attach to network");
            let _response = bg77
                .long_call_at("+CGATT=1", ACTIVATION_TIMEOUT + Duration::from_secs(1))
                .await
                .map_err(RegistrationError::from_error)?;
            #[cfg(feature = "defmt")]
            if !_response.lines().is_empty() {
                debug!("Read {=[?]} after CGATT=1", _response.lines());
            }
        }

        let (_, stat) = bg77.call_at("+CGACT?", None).await?.parse2::<u8, u8>([0, 1], Some(1))?;
        if stat != 1 {
            let _ = bg77
                .long_call_at("+CGACT=1,1", ACTIVATION_TIMEOUT)
                .await
                .map_err(RegistrationError::from_error)?;
            let res = bg77.call_at("+CGACT?", None).await;
            let stat_retry = match res {
                Ok(r) => r.parse2::<u8, u8>([0, 1], Some(1)).map(|(_, s)| s).unwrap_or(0),
                Err(err) => return Err(RegistrationError::from_error(err).into()),
            };
            if stat_retry != 1 {
                return Err(RegistrationError::PdpContextFailed.into());
            }
        } else if att_state == 1 {
            info!("Already registered to network");
        }

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
        let mut config = ModemConfig {
            apn: String::from_str("test-apn").unwrap(),
            ..Default::default()
        };
        config.bands.set_ltem_bands(&[3]);
        let modem_manager = ModemManager::<FakeModem>::new(config);

        let mut bg77 = FakeModem::new(&[
            ("AT+QGMR", "fake-firmware"),
            ("AT+CGDCONT=1,\"IP\",\"test-apn\"", ""),
            ("AT+CEREG=2", ""),
            ("AT+QCFG=\"nwscanseq\",00", ""),
            ("AT+QCFG=\"iotopmode\",2,1", ""),
            ("AT+QCFG=\"band\",0,4,80000", ""),
            ("AT+CGATT=1", "+CME ERROR: 30"),
        ]);
        let firmware = block_on(modem_manager.configure(&mut bg77));
        assert!(firmware.is_ok());
        assert!(bg77.all_done());
    }

    #[test]
    fn test_network_registration_cgatt_error_fails() {
        let modem_manager = ModemManager::<FakeModem>::new(ModemConfig::default());
        let mut bg77 =
            FakeModem::new(&[("AT+CGATT?", "+CGATT: 0"), ("AT+CGATT=1", "+CME ERROR: 30")]);
        let res = block_on(modem_manager.network_registration(&mut bg77, false));
        assert_eq!(res, Err(RegistrationError::NoNetworkService.into()));
        assert!(bg77.all_done());
    }

    #[test]
    fn test_network_registration_cgact_error_fails() {
        let modem_manager = ModemManager::<FakeModem>::new(ModemConfig::default());
        let mut bg77 = FakeModem::new(&[
            ("AT+CGATT?", "+CGATT: 1"),
            ("AT+CGACT?", "+CGACT: 1,0"),
            ("AT+CGACT=1,1", "+CME ERROR: 31"),
        ]);
        let res = block_on(modem_manager.network_registration(&mut bg77, false));
        assert_eq!(res, Err(RegistrationError::NetworkTimeout.into()));
        assert!(bg77.all_done());
    }

    #[test]
    fn test_network_registration_deactivated_context_fails() {
        let modem_manager = ModemManager::<FakeModem>::new(ModemConfig::default());
        let mut bg77 = FakeModem::new(&[
            ("AT+CGATT?", "+CGATT: 0"),
            ("AT+CGATT=1", ""),
            ("AT+CGACT?", "+CGACT: 1,0"),
            ("AT+CGACT=1,1", ""),
            ("AT+CGACT?", "+CGACT: 1,0"),
        ]);
        let res = block_on(modem_manager.network_registration(&mut bg77, false));
        assert_eq!(res, Err(RegistrationError::PdpContextFailed.into()));
        assert!(bg77.all_done());
    }

    #[test]
    fn test_network_registration_success() {
        let modem_manager = ModemManager::<FakeModem>::new(ModemConfig::default());
        let mut bg77 = FakeModem::new(&[
            ("AT+CGATT?", "+CGATT: 0"),
            ("AT+CGATT=1", ""),
            ("AT+CGACT?", "+CGACT: 1,1"),
        ]);
        let res = block_on(modem_manager.network_registration(&mut bg77, false));
        assert!(res.is_ok());
        assert!(bg77.all_done());
    }

    #[test]
    fn test_network_registration_already_registered_reactivates_context() {
        let modem_manager = ModemManager::<FakeModem>::new(ModemConfig::default());
        let mut bg77 = FakeModem::new(&[
            ("AT+CGATT?", "+CGATT: 1"),
            ("AT+CGACT?", "+CGACT: 1,0"),
            ("AT+CGACT=1,1", ""),
            ("AT+CGACT?", "+CGACT: 1,1"),
        ]);
        let res = block_on(modem_manager.network_registration(&mut bg77, false));
        assert!(res.is_ok());
        assert!(bg77.all_done());
    }
}
