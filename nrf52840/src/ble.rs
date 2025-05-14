use core::fmt::Write;
use embassy_executor::Spawner;
use heapless::String;
use nrf_softdevice::{ble, raw, temperature_celsius, Flash, Softdevice};
use yaroc_common::error::Error;

pub struct Ble {
    inner: &'static Softdevice,
}

impl Ble {
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
            inner: Softdevice::enable(&config),
        }
    }

    pub fn must_spawn(&self, spawner: Spawner) {
        spawner.must_spawn(softdevice_task(self.inner));
    }

    pub fn get_mac_address(&self) -> String<12> {
        let bytes = ble::get_address(self.inner).bytes();
        let mut res = String::new();
        for b in bytes.iter().rev() {
            write!(&mut res, "{:02x}", b).expect("Unexpected length, this is a bug");
        }
        res
    }

    pub fn temperature(&self) -> crate::Result<f32> {
        temperature_celsius(self.inner)
            .map(|val| val.to_num::<f32>())
            //TODO: consider propagating the error code
            .map_err(|_| Error::SoftdeviceError)
    }

    pub fn flash(&self) -> Flash {
        Flash::take(self.inner)
    }
}

#[cfg(feature = "bluetooth-le")]
#[embassy_executor::task]
async fn softdevice_task(sd: &'static Softdevice) -> ! {
    sd.run().await
}
