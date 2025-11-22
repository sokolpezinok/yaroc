use core::str::FromStr;

use crate::at::response::{AT_COMMAND_SIZE, AT_LINES, AtResponse, CommandResponse, FromModem};
use crate::at::uart::{AtUartTrait, UrcHandlerType};
use embassy_executor::Spawner;
use embassy_time::Duration;
use heapless::{String, Vec, format, index_map::FnvIndexMap};

pub trait ModemHw: AtUartTrait {
    /// Default timeout for a command. It's typically the minimum timeout, a couple hundred
    /// milliseconds for a modem.
    const DEFAULT_TIMEOUT: Duration;

    /// Spawn a task for the modem and process incoming URCs using the provided handlers.
    fn spawn(&mut self, spawner: Spawner, urc_handlers: &[UrcHandlerType]) {
        self.spawn_rx(urc_handlers, spawner);
    }

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

    /// Reads an AT response from the modem.
    fn read(
        &mut self,
        cmd: &str,
        timeout: Duration,
    ) -> impl Future<Output = crate::Result<AtResponse>> {
        async move {
            let lines = AtUartTrait::read(self, timeout).await?;
            Ok(AtResponse::new(lines, cmd))
        }
    }
}

pub struct FakeModem {
    responses: FnvIndexMap<String<AT_COMMAND_SIZE>, String<60>, 8>,
}

impl FakeModem {
    pub fn new(interactions: &[(&str, &str)]) -> Self {
        let mut responses = FnvIndexMap::new();
        for (command, response) in interactions {
            responses
                .insert(
                    String::from_str(command).unwrap(),
                    String::from_str(response).unwrap(),
                )
                .unwrap();
        }
        Self { responses }
    }
}

impl AtUartTrait for FakeModem {
    fn spawn_rx(&mut self, _urc_handlers: &[UrcHandlerType], _spawner: Spawner) {}

    async fn call_at_timeout(
        &mut self,
        command: &str,
        _call_timeout: Duration,
        _response_timeout: Option<Duration>,
    ) -> crate::Result<AtResponse> {
        let at_cmd = format!(AT_COMMAND_SIZE; "AT{command}").unwrap();
        let command_response =
            CommandResponse::new(self.responses.get(at_cmd.as_str()).unwrap()).unwrap();
        let response = FromModem::CommandResponse(command_response);
        Ok(AtResponse::new([response, FromModem::Ok].into(), command))
    }

    async fn call_second_read(
        &mut self,
        _msg: &[u8],
        _command_prefix: &str,
        _second_read: bool,
        _timeout: Duration,
    ) -> crate::Result<AtResponse> {
        todo!()
    }

    async fn read(&self, _timeout: Duration) -> crate::Result<Vec<FromModem, AT_LINES>> {
        todo!()
    }
}

impl ModemHw for FakeModem {
    const DEFAULT_TIMEOUT: Duration = Duration::from_millis(10);
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
