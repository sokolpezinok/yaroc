use embassy_nrf::uarte::UarteRxWithIdle;
use embassy_sync::channel::Sender;
use yaroc_common::{RawMutex, backoff::BatchedPunches, error::Error, si_uart::SiUart};

/// A task that reads punches from the SI-UART and sends them to a channel.
///
/// This task is designed to run continuously, reading punches from the `si_uart`
/// and sending them to the `punch_sender` channel. This decouples the reading of
/// punches from their processing, which is important because the processing might
/// involve waiting for the modem, which can be a long operation.
#[embassy_executor::task]
pub async fn read_si_uart(
    mut si_uart: SiUart<UarteRxWithIdle<'static>>,
    punch_sender: Sender<'static, RawMutex, Result<BatchedPunches, Error>, 24>,
) {
    loop {
        match si_uart.read_grouped_punches().await {
            Err(err) => punch_sender.send(Err(err)).await,
            Ok(grouped_punches) => {
                for punches in grouped_punches {
                    punch_sender.send(Ok(punches)).await;
                }
            }
        }
    }
}
