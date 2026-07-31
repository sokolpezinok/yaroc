extern crate std;

use crate::at::{
    response::{AT_COMMAND_SIZE, AT_LINES, AtResponse, FromModem},
    uart::{AtUartTrait, UrcHandlerType},
};
use core::str::FromStr;
use embassy_executor::Spawner;
use embassy_time::Duration;
use heapless::{String, Vec};

pub struct FakeModem {
    at_responses: std::vec::Vec<(String<AT_COMMAND_SIZE>, String<60>)>,
    responses: std::vec::Vec<(String<AT_COMMAND_SIZE>, bool, String<60>)>,
}

impl FakeModem {
    pub fn new(at_interactions: &[(&str, &str)]) -> Self {
        let mut at_responses = std::vec::Vec::new();
        for (command, response) in at_interactions {
            at_responses.push((
                String::from_str(command).unwrap(),
                String::from_str(response).unwrap(),
            ));
        }
        Self {
            at_responses,
            responses: Default::default(),
        }
    }

    pub fn add_pure_interactions(&mut self, interactions: &[(&str, bool, &str)]) {
        let mut responses = std::vec::Vec::new();
        for (command, second_read, at_response) in interactions {
            responses.push((
                String::from_str(command).unwrap(),
                *second_read,
                String::from_str(at_response).unwrap(),
            ));
        }
        self.responses = responses;
    }

    pub fn all_done(&self) -> bool {
        self.at_responses.is_empty() && self.responses.is_empty()
    }
}

impl AtUartTrait for FakeModem {
    const DEFAULT_TIMEOUT: Duration = Duration::from_millis(10);

    fn spawn_rx(&mut self, _urc_handlers: &[UrcHandlerType], _spawner: Spawner) {}

    async fn call_at_timeout(
        &mut self,
        command: &str,
        _call_timeout: Duration,
        _response_timeout: Option<Duration>,
    ) -> crate::Result<AtResponse> {
        let (at_cmd, at_response_raw) = self.at_responses.remove(0);
        assert_eq!(
            at_cmd.as_str(),
            std::format!("AT{command}"),
            "Expected {at_cmd}, got AT{command}"
        );
        let mut responses: Vec<FromModem, AT_LINES> = Vec::new();
        if !at_response_raw.is_empty() {
            for line in at_response_raw.lines() {
                if line.is_empty() {
                    continue;
                }
                responses
                    .push(FromModem::try_from(line).unwrap())
                    .expect("Fake modem responses too short");
            }
        }
        if !responses.iter().any(|r| r.terminal()) {
            responses.push(FromModem::Ok).expect("Fake modem responses too short");
        }
        Ok(AtResponse::new(responses, command))
    }

    async fn call_second_read(
        &mut self,
        _msg: &[u8],
        command_prefix: &str,
        second_read: bool,
        _timeout: Duration,
    ) -> crate::Result<AtResponse> {
        let (expected_cmd, expected_read, at_response) = self.responses.remove(0);
        assert_eq!(expected_cmd, command_prefix);
        assert_eq!(expected_read, second_read);
        let from_modem = FromModem::try_from(at_response.as_str()).unwrap();
        Ok(AtResponse::new(
            [from_modem, FromModem::Eof].into(),
            command_prefix,
        ))
    }

    async fn read_lines(&self, _timeout: Duration) -> crate::Result<Vec<FromModem, AT_LINES>> {
        todo!()
    }
}
