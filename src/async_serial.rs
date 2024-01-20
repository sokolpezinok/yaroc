use std::{collections::HashMap, error::Error, time::Duration};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

pub struct AsyncSerial {
    serial: tokio::sync::Mutex<SerialStream>,
    buffer_size: usize,
}

#[derive(thiserror::Error, Debug)]
pub enum AsyncSerialError {
    #[error("Timed out")]
    Timeout(f64), // timeout in seconds
    #[error("Modem error")]
    ModemError,
    #[error("Serial read error")]
    ReadError(Box<dyn Error>),
    #[error("Serial write error")]
    WriteError(Box<dyn Error>),
}

impl AsyncSerial {
    pub fn new(port: &str) -> Result<Self, serialport::Error> {
        let builder = tokio_serial::new(port, 115200);
        let serial = builder.open_native_async()?;
        Ok(Self {
            serial: tokio::sync::Mutex::new(serial),
            buffer_size: 1024,
        })
    }

    pub async fn call(
        &mut self,
        command: &str,
        matc: &str,
        queries: &[&str],
        timeout: f64,
    ) -> Result<HashMap<String, String>, AsyncSerialError> {
        let response = self.call_with_timeout(command, timeout).await?;
        if response.len() == 0 || response.last().unwrap() == "ERROR" {
            return Err(AsyncSerialError::ModemError);
        }
        let query = Self::search(response, matc, queries);
        Ok(query)
    }

    pub async fn call_without_match(
        &mut self,
        command: &str,
        timeout: f64,
    ) -> Result<(), AsyncSerialError> {
        let response = self.call_with_timeout(command, timeout).await?;
        if response.len() == 0 || response.last().unwrap() == "ERROR" {
            return Err(AsyncSerialError::ModemError);
        }
        Ok(())
    }

    async fn call_with_timeout(
        &mut self,
        command: &str,
        timeout: f64,
    ) -> Result<Vec<String>, AsyncSerialError> {
        // TODO: read what's left in the read buffer in the beginning
        let mut serial = self.serial.lock().await;
        serial
            .write(format!("{command}\r\n").as_bytes())
            .await
            .map_err(|e| AsyncSerialError::WriteError(Box::new(e)))?;

        let mut buffer = Vec::with_capacity(self.buffer_size);
        let result = tokio::time::timeout(
            Duration::from_micros((timeout * 1_000_000.0).trunc() as u64),
            serial.read_buf(&mut buffer), // TODO: this should stop on OK or ERROR
        )
        .await;
        std::mem::drop(serial);

        match result {
            Ok(read_result) => match read_result {
                Ok(_) => {
                    let buffer = String::from_utf8(buffer)
                        .map_err(|e| AsyncSerialError::ReadError(Box::new(e)))?;
                    Ok(buffer
                        .split("\r\n")
                        .filter(|line| !line.is_empty())
                        .map(|line| line.trim_end().to_owned())
                        .collect())
                }
                Err(e) => Err(AsyncSerialError::ReadError(Box::new(e))),
            },
            Err(_) => Err(AsyncSerialError::Timeout(timeout)),
        }
    }

    fn search(lines: Vec<String>, needle: &str, ids: &[&str]) -> HashMap<String, String> {
        let re = regex::Regex::new(needle).unwrap();
        for line in lines.into_iter() {
            if let Some(c) = re.captures(&line) {
                return ids
                    .iter()
                    .map(|key| ((*key).to_owned(), c[*key].to_owned()))
                    .collect();
            }
        }
        HashMap::new()
    }
}

#[cfg(test)]
mod test_search {
    use super::AsyncSerial;

    #[test]
    fn test_search() {
        let res = AsyncSerial::search(
            vec!["CENG: 1,2,3,\"abc\",1,2,-89,".to_string()],
            r#"CENG: .*,.*,.*,"(?<cell>.*)",.*,.*,(?<rssi>.*),"#,
            &["cell", "rssi"],
        );
        assert_eq!(res.get("cell"), Some(&"abc".to_owned()));
        assert_eq!(res.get("rssi"), Some(&"-89".to_owned()));
    }
}
