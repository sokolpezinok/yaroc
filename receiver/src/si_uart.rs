use std::collections::HashMap;
use std::collections::hash_map::Entry;

use log::{error, warn};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tokio_util::sync::CancellationToken;

use yaroc_common::punch::RawPunch;
use yaroc_common::si_uart::SiUart;
use yaroc_common::{error::Error, si_uart::RxWithIdle};

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
        let builder = tokio_serial::new(port, 38400);
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

/// An SportIdent UART device handler
///
/// This handler connects to a SportIdent UART device and forwards punches to the punch transmitter.
pub struct SiUartHandler {
    cancellation_tokens: HashMap<String, CancellationToken>,
    punch_tx: UnboundedSender<RawPunch>,
}

//TODO: consider merging logic with MshDevHandler
impl SiUartHandler {
    /// Creates a new `SiUartHandler`.
    ///
    /// # Returns
    ///
    /// A tuple containing the new `SiUartHandler` instance and a receiver for punches.
    pub fn new() -> (Self, UnboundedReceiver<RawPunch>) {
        let (punch_tx, punch_rx) = unbounded_channel();
        (
            Self {
                cancellation_tokens: HashMap::new(),
                punch_tx,
            },
            punch_rx,
        )
    }
    /// Connects to a SportIdent UART device.
    ///
    /// This function spawns a task to handle messages from the device.
    ///
    /// # Arguments
    ///
    /// * `serial` - The serial port to use.
    /// * `device_node` - A string identifying the device, usually its path.
    pub fn add_device(&mut self, serial: TokioSerial, device_node: &str) {
        let port = serial.port().to_owned();
        let token = self.spawn_serial(serial, port);
        self.cancellation_tokens.insert(device_node.to_owned(), token);
    }

    /// Disconnects a SportIdent UART device.
    ///
    /// This function cancels the task that handles messages from the device and returns true if
    /// the device was connected.
    ///
    /// # Arguments
    ///
    /// * `device_node` - A string identifying the device, usually its path.
    pub fn remove_device(&mut self, device_node: String) -> bool {
        if let Entry::Occupied(occupied_entry) = self.cancellation_tokens.entry(device_node) {
            occupied_entry.get().cancel();
            occupied_entry.remove();
            true
        } else {
            false
        }
    }

    /// Spawns a task to read punches from a SportIdent UART serial connection.
    ///
    /// The task forwards the messages to the punch transmitter and can be cancelled by the returned
    /// `CancellationToken`.
    fn spawn_serial(&mut self, serial: TokioSerial, port: String) -> CancellationToken {
        let cancellation_token = CancellationToken::new();
        let cancellation_token_clone = cancellation_token.clone();
        let punch_tx = self.punch_tx.clone();
        let mut si_uart = SiUart::new(serial);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        warn!("Stopping SI UART device: {port}");
                        break;
                    }
                    punch = si_uart.read() => {
                        match punch {
                            Ok(punches) => {
                                for punch in punches {
                                    punch_tx
                                        .send(punch)
                                        .expect("Channel unexpectedly closed");
                                }
                            }
                            Err(err) => {
                                match err {
                                    Error::UartClosedError => {
                                        error!("Device removed: {port}");
                                        cancellation_token.cancel();
                                        break;
                                    }
                                    e => {
                                        error!("Failed to read punch: {e}");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
        cancellation_token_clone
    }
}
