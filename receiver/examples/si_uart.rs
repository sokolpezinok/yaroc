//! An example of reading punches from a SportIdent UART device.

use chrono::Local;
use log::{error, info};

use yaroc_common::punch::SiPunch;
use yaroc_receiver::usb_serial_manager::{SportIdentMessage, UsbSerialManager};

#[tokio::main]
async fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .format_timestamp_millis()
        .init();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut manager = UsbSerialManager::new(None, Some(tx));

    let monitor_task = tokio::spawn(async move {
        if let Err(e) = manager.monitor_usb_devices().await {
            error!("Error in USB monitoring: {e}");
        }
    });

    info!("Watching for SportIdent USB devices... Press Ctrl-C to exit.");
    loop {
        tokio::select! {
            res = rx.recv() => {
                match res {
                    Some(SportIdentMessage::RawPunch(punch)) => {
                        let now = Local::now();
                        let punch = SiPunch::from_raw(punch, now.date_naive(), now.offset());
                        info!("Received punch: {punch:?}");
                    }
                    Some(SportIdentMessage::DeviceEvent { added, device }) => {
                        info!("Device event: added={added}, device={device}");
                    }
                    None => {
                        info!("Channel closed, shutting down...");
                        break;
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl-C received, shutting down...");
                monitor_task.abort();
                break;
            }
        }
    }
}
