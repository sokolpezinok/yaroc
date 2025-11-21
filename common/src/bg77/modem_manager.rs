use heapless::format;

use crate::bg77::hw::{ACTIVATION_TIMEOUT, ModemConfig, ModemHw, RAT};
use crate::error::Error;

pub struct ModemManager {
    config: ModemConfig,
}

impl ModemManager {
    pub fn new(config: ModemConfig) -> Self {
        Self { config }
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
