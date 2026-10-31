MEMORY {
  /* NOTE 1 K = 1 KiBi = 1024 bytes */
  /* NRF52840 with Softdevice S140 7.3.0 */
  /* Bootloader reserves the last 48K, so we only have 976K of flash available */
  /* Softdevice S140 reserves 156K flash in the beginning and 31K of RAM. */
  /* The 564K between Softdevice (156K) and DATA_FLASH (720K) is split into: */
  /*   FLASH (Active): 276K, DFU: 280K (Active + 4K), BOOTLOADER_STATE: 8K */
  FLASH : ORIGIN = 0x0 + 156K, LENGTH = 276K
  DFU : ORIGIN = 0x0 + 156K + 276K, LENGTH = 280K
  BOOTLOADER_STATE : ORIGIN = 0x0 + 976K - 256K - 8K, LENGTH = 8K
  DATA_FLASH : ORIGIN = 0x0 + 976K - 256K, LENGTH = 256K
  RAM : ORIGIN = 0x20000000 + 31K, LENGTH = 256K - 31K
}

__bootloader_state_start = ORIGIN(BOOTLOADER_STATE);
__bootloader_state_end = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE);
__bootloader_active_start = ORIGIN(FLASH);
__bootloader_active_end = ORIGIN(FLASH) + LENGTH(FLASH);
__bootloader_dfu_start = ORIGIN(DFU);
__bootloader_dfu_end = ORIGIN(DFU) + LENGTH(DFU);

SECTIONS {
  PROVIDE(_data_flash_start = ORIGIN(DATA_FLASH));
  PROVIDE(_data_flash_size = LENGTH(DATA_FLASH));
}
