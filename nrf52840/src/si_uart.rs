use defmt::{error, info};
use embassy_nrf::uarte::UarteRxWithIdle;
use embassy_time::Instant;
use heapless::format;
use yaroc_common::{
    at::response::{FLASH_LOG_CHANNEL, FlashLog},
    backoff::{BackoffCommand, CMD_FOR_BACKOFF},
    bg77::system_info,
    punch::{LoggedPunch, SiPunch},
    si_uart::SiUart,
};

/// A task that reads punches from the SI-UART and publishes them for backoff retries.
///
/// This task is designed to run continuously, reading punches from the `si_uart`
/// and sending them directly to `CMD_FOR_BACKOFF`. This decouples the reading of
/// punches from their processing, which is important because the processing might
/// involve waiting for the modem, which can be a long operation.
#[embassy_executor::task]
pub async fn read_si_uart(mut si_uart: SiUart<UarteRxWithIdle<'static>>) {
    let mut punch_cnt = 0;
    loop {
        match si_uart.read_grouped_punches().await {
            Ok(grouped_punches) => {
                let time = system_info::time_from_instant(Instant::now());
                for punches in grouped_punches {
                    let punch_id = punch_cnt;
                    CMD_FOR_BACKOFF
                        .send(BackoffCommand::PublishPunches(punches.clone(), punch_id))
                        .await;
                    punch_cnt += 1;

                    let today = time.date_naive();
                    for punch in punches {
                        let _ = FLASH_LOG_CHANNEL
                            .try_send(FlashLog::Punch(LoggedPunch::new(time, punch)))
                            .inspect_err(|e| error!("Failed to send punch for logging: {:?}", e));
                        let punch = SiPunch::from_raw(punch, today, time.offset());
                        info!(
                            "{} punched {} at {}, ID={}",
                            punch.card,
                            punch.code,
                            format!(40; "{}", punch.time).unwrap(),
                            punch_id,
                        );
                    }
                }
            }
            Err(err) => {
                error!("Error while receiving punches: {}", err);
            }
        }
    }
}
