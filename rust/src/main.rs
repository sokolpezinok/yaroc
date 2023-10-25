use std::{collections::HashMap, error::Error, time::Duration};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

struct AsyncSerial {
    serial: tokio::sync::Mutex<SerialStream>,
}

#[derive(thiserror::Error, Debug)]
enum AsyncSerialError {
    #[error("Timed out")]
    Timeout(f64), // timeout in seconds
    #[error("Serial read error")]
    ReadError(Box<dyn Error>),
    #[error("Serial write error")]
    WriteError(Box<dyn Error>),
}

impl AsyncSerial {
    fn new(port: &str) -> Result<Self, serialport::Error> {
        let builder = tokio_serial::new(port, 115200);
        let serial = builder.open_native_async()?;
        Ok(Self {
            serial: tokio::sync::Mutex::new(serial),
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
        let query = Self::search(response, matc, queries);
        Ok(query)
    }

    async fn call_with_timeout(
        &mut self,
        command: &str,
        timeout: f64,
    ) -> Result<Vec<String>, AsyncSerialError> {
        let mut serial = self.serial.lock().await;
        serial
            .write(format!("{command}\r\n").as_bytes())
            .await
            .map_err(|e| AsyncSerialError::WriteError(Box::new(e)))?;

        let mut buffer = Vec::with_capacity(256);
        let result = tokio::time::timeout(
            Duration::from_micros((timeout * 1_000_000.0).trunc() as u64),
            serial.read_buf(&mut buffer),
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
                        .map(|line| line.to_owned())
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

#[tokio::main]
async fn main() {
    let mut serial = AsyncSerial::new("/dev/ttyUSB2").unwrap();
    let b = serial
        .call(
            "AT+CPSI?",
            r"\+CPSI: (?<serv>.*),(?<stat>.*)",
            &["serv", "stat"],
            1.0,
        )
        .await;
    println!("{:?}", b);
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
