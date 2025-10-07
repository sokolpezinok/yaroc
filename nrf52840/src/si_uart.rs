//! SI-UART driver.
//!
//! This module is a bit of a misnomer. It doesn't implement the full SI-UART protocol, but rather
//! a simplified version that reads punches from a SportIdent device. It is designed to be used
//! with a single SportIdent device connected to the UART.
//!
//! The module provides a task that reads from the UART and sends the punches to a channel.

use crate::error::Error;
use embassy_nrf::uarte::UarteRxWithIdle;
use embassy_sync::channel::{Channel, Sender};
use yaroc_common::{RawMutex, punch::RawPunch, si_uart::SiUart};

/// A channel for sending punches from the SI UART to the event handler.
pub type SiUartChannelType = Channel<RawMutex, Result<RawPunch, Error>, 40>;

/// A task that reads from the SI UART and sends punches to a channel.
///
/// This task continuously reads from the SI-UART and sends the parsed punches to the provided
/// channel. If an error occurs during reading or parsing, the error is sent to the channel.
#[embassy_executor::task]
pub async fn si_uart_reader(
    mut si_uart: SiUart<UarteRxWithIdle<'static>>,
    punch_sender: Sender<'static, RawMutex, Result<RawPunch, Error>, 40>,
) {
    loop {
        match si_uart.read().await {
            Err(err) => {
                punch_sender.send(Err(err)).await;
            }
            Ok(buffer) => {
                punch_sender.send(Ok(buffer)).await;
            }
        }
    }
}
