use std::{error::Error, time::Duration};

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
}

#[tokio::main]
async fn main() {
    let mut serial = AsyncSerial::new("/dev/ttyUSB2").unwrap();
    let b = serial.call_with_timeout("AT+CPSI?", 1.0).await;
    println!("{:?}", b);
}
