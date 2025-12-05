use crate::at::{response::AtResponse, uart::AtUartTrait};
use embassy_time::Duration;

pub trait ModemHw: AtUartTrait {
    /// Default timeout for a command. It's typically the minimum timeout, a couple hundred
    /// milliseconds for a modem.
    const DEFAULT_TIMEOUT: Duration;

    /// Performs an AT call to the modem, optionally also waiting longer for a response.
    ///
    /// We send `cmd` prefixed by `AT`. We wait a short time for an OK/ERROR and then if
    /// `response_timeout` is set, we wait `response_timeout` for a response that is prefixed by
    /// `cmd`.
    fn call_at(
        &mut self,
        cmd: &str,
        response_timeout: Option<Duration>,
    ) -> impl Future<Output = crate::Result<AtResponse>> {
        self.call_at_timeout(cmd, Self::DEFAULT_TIMEOUT, response_timeout)
    }

    /// Performs an AT call to the modem and waits for an OK/ERROR response.
    ///
    /// The maximum waiting time is specified by `timeout`.
    fn long_call_at(
        &mut self,
        cmd: &str,
        timeout: Duration,
    ) -> impl Future<Output = crate::Result<AtResponse>> {
        self.call_at_timeout(cmd, timeout, None)
    }

    /// Sends a raw message to the modem.
    ///
    /// Waits for a response if `second_read_timeout` is set and the timeout is the value of
    /// `second_read_timeout`. The response should be prefixed with `command_prefix`.
    fn call(
        &mut self,
        msg: &[u8],
        command_prefix: &str,
        second_read_timeout: Option<Duration>,
    ) -> impl Future<Output = crate::Result<AtResponse>> {
        match second_read_timeout {
            None => self.call_second_read(msg, command_prefix, false, Self::DEFAULT_TIMEOUT),
            Some(timeout) => self.call_second_read(msg, command_prefix, true, timeout),
        }
    }

    /// Reads a response from the modem.
    fn read(&mut self, timeout: Duration) -> impl Future<Output = crate::Result<AtResponse>> {
        async move {
            let lines = AtUartTrait::read(self, timeout).await?;
            Ok(AtResponse::new(lines, ""))
        }
    }
}

#[cfg(feature = "nrf")]
/// Minimum timeout for BG77 AT-command responses.
static BG77_MINIMUM_TIMEOUT: Duration = Duration::from_millis(300);

#[cfg(feature = "nrf")]
impl ModemHw
    for crate::at::uart::AtUart<
        embassy_nrf::uarte::UarteTx<'static>,
        embassy_nrf::uarte::UarteRxWithIdle<'static>,
    >
{
    const DEFAULT_TIMEOUT: Duration = BG77_MINIMUM_TIMEOUT;
}
