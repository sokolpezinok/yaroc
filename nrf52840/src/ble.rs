use core::fmt::Write;
use embassy_executor::Spawner;
use heapless::String;
use nrf_softdevice::{Flash, Softdevice, ble, raw, temperature_celsius};
use yaroc_common::error::Error;

/// Bluetooth Low Energy (BLE) stack.
///
/// This struct manages the nrf-softdevice configuration and initialization.
pub struct Ble {
    softdevice: &'static Softdevice,
}

impl Default for Ble {
    /// Creates a new `Ble` instance with default configuration.
    fn default() -> Self {
        Self::new()
    }
}

impl Ble {
    /// Creates a new `Ble` instance and enables the Softdevice with a custom configuration.
    pub fn new() -> Self {
        let config = nrf_softdevice::Config {
            clock: Some(raw::nrf_clock_lf_cfg_t {
                source: raw::NRF_CLOCK_LF_SRC_RC as u8,
                rc_ctiv: 16,
                rc_temp_ctiv: 2,
                accuracy: raw::NRF_CLOCK_LF_ACCURACY_500_PPM as u8,
            }),
            conn_gap: Some(raw::ble_gap_conn_cfg_t {
                conn_count: 6,
                event_length: 24,
            }),
            conn_gatt: Some(raw::ble_gatt_conn_cfg_t { att_mtu: 256 }),
            gatts_attr_tab_size: Some(raw::ble_gatts_cfg_attr_tab_size_t {
                attr_tab_size: raw::BLE_GATTS_ATTR_TAB_SIZE_DEFAULT,
            }),
            gap_role_count: Some(raw::ble_gap_cfg_role_count_t {
                adv_set_count: 1,
                periph_role_count: 1,
                central_role_count: 0,
                central_sec_count: 0,
                _bitfield_1: raw::ble_gap_cfg_role_count_t::new_bitfield_1(0),
            }),
            gap_device_name: Some(raw::ble_gap_cfg_device_name_t {
                p_value: b"YAROC" as *const u8 as _,
                current_len: 5,
                max_len: 5,
                write_perm: unsafe { core::mem::zeroed() },
                _bitfield_1: raw::ble_gap_cfg_device_name_t::new_bitfield_1(
                    raw::BLE_GATTS_VLOC_STACK as u8,
                ),
            }),
            ..Default::default()
        };

        Self {
            softdevice: Softdevice::enable(&config),
        }
    }

    /// Spawns the Softdevice task.
    ///
    /// This is required for the Softdevice to run.
    pub fn must_spawn(&self, spawner: Spawner) {
        spawner.must_spawn(softdevice_task(self.softdevice));
    }

    /// Returns the MAC address of the device as a hex string.
    pub fn get_mac_address(&self) -> String<12> {
        let bytes = ble::get_address(self.softdevice).bytes();
        let mut res = String::new();
        for b in bytes.iter().rev() {
            write!(&mut res, "{:02x}", b).expect("Unexpected length, this is a bug");
        }
        res
    }

    /// Returns the temperature of the CPU.
    pub fn temperature(&self) -> crate::Result<f32> {
        temperature_celsius(self.softdevice)
            .map(|val| val.to_num::<f32>())
            //TODO: consider propagating the error code
            .map_err(|_| Error::SoftdeviceError)
    }

    /// Returns a handle to the Softdevice's flash memory.
    pub fn flash(&self) -> Flash {
        Flash::take(self.softdevice)
    }
}

/// The main task for the Softdevice.
///
/// This task runs the Softdevice and must be spawned for BLE to work.
#[embassy_executor::task]
async fn softdevice_task(sd: &'static Softdevice) -> ! {
    sd.run().await
}
