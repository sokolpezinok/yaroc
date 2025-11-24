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
    at_responses: FnvIndexMap<String<AT_COMMAND_SIZE>, String<60>, 8>,
    responses: FnvIndexMap<String<AT_COMMAND_SIZE>, (bool, String<60>), 2>,
}

impl FakeModem {
    pub fn new(at_interactions: &[(&str, &str)]) -> Self {
        let mut at_responses = FnvIndexMap::new();
        for (command, response) in at_interactions {
            at_responses
                .insert(
                    String::from_str(command).unwrap(),
                    String::from_str(response).unwrap(),
                )
                .unwrap();
        }
        Self {
            at_responses,
            responses: Default::default(),
        }
    }

    pub fn add_pure_interactions(&mut self, interactions: &[(&str, bool, &str)]) {
        let mut responses = FnvIndexMap::new();
        for (command, second_read, at_response) in interactions {
            responses
                .insert(
                    String::from_str(command).unwrap(),
                    (*second_read, String::from_str(at_response).unwrap()),
                )
                .unwrap();
        }
        self.responses = responses;
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
        let at_response_raw = self.at_responses.get(at_cmd.as_str()).unwrap();
        let responses: Vec<_, _> = if at_response_raw.is_empty() {
            [FromModem::Ok].into()
        } else {
            [
                FromModem::CommandResponse(CommandResponse::new(at_response_raw.as_str()).unwrap()),
                FromModem::Ok,
            ]
            .into()
        };
        Ok(AtResponse::new(responses, command))
    }

    async fn call_second_read(
        &mut self,
        _msg: &[u8],
        command_prefix: &str,
        second_read: bool,
        _timeout: Duration,
    ) -> crate::Result<AtResponse> {
        let (expected_read, at_response) =
            self.responses.get(command_prefix).expect("Unexpected call");
        assert_eq!(*expected_read, second_read);
        let response = CommandResponse::new(at_response.as_str()).unwrap();
        Ok(AtResponse::new(
            [FromModem::CommandResponse(response), FromModem::Eof].into(),
            command_prefix,
        ))
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
