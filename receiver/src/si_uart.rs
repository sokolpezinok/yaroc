use log::{error, warn};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tokio_util::sync::CancellationToken;

use yaroc_common::punch::RawPunch;
use yaroc_common::si_uart::{BAUD_RATE, SiUart};
use yaroc_common::{error::Error, si_uart::RxWithIdle};

use crate::serial_device_manager::UsbSerialTrait;

pub struct TokioSerial {
    serial: SerialStream,
    port: String,
}

impl TokioSerial {
    /// Creates a new `TokioSerial` instance from a serial port path.
    ///
    /// # Arguments
    ///
    /// * `port` - The path to the serial port, e.g., `/dev/ttyUSB0`.
    ///
    /// # Returns
    ///
    /// A `Result` containing the new `TokioSerial` instance or an error if the port cannot be
    /// opened.
    pub fn new(port: &str) -> crate::Result<Self> {
        let builder = tokio_serial::new(port, BAUD_RATE);
        let serial =
            builder.open_native_async().map_err(|_| crate::error::Error::ConnectionError)?;
        Ok(Self {
            serial,
            port: port.to_owned(),
        })
    }

    /// Returns the port path of the serial connection.
    pub fn port(&self) -> &str {
        &self.port
    }
}

impl RxWithIdle for TokioSerial {
    /// Reads from the serial port until a timeout is hit, which is considered to be an idle state.
    ///
    /// This is a thin wrapper around `tokio_serial::SerialStream::read`.
    async fn read_until_idle(&mut self, buf: &mut [u8]) -> yaroc_common::Result<usize> {
        self.serial.read(buf).await.map_err(|_| Error::UartReadError)
    }
}

impl UsbSerialTrait for SiUart<TokioSerial> {
    type Output = RawPunch;

    async fn inner_loop(
        mut self,
        cancellation_token: CancellationToken,
        tx: UnboundedSender<Self::Output>,
    ) {
        let port = self.rx.port().to_owned();
        let output = cancellation_token
            .run_until_cancelled_owned(async {
                loop {
                    let punch = self.read().await;
                    match punch {
                        Ok(punches) => {
                            for punch in punches {
                                tx.send(punch).expect("Channel unexpectedly closed");
                            }
                        }
                        Err(err) => match err {
                            Error::UartClosedError => {
                                error!("Removed SI UART device: {port}");
                                break;
                            }
                            e => {
                                error!("Failed to read punch: {e}");
                            }
                        },
                    }
                }
            })
            .await;
        if output.is_none() {
            warn!("Stopping SI UART device: {port}");
        }
    }
}
