MEMORY {
  /* NOTE 1 K = 1 KiBi = 1024 bytes */
  /* NRF52840 with Softdevice S140 7.3.0 */
  /* RAK bootloader reserves the last 48K, so we only have 976K of flash available */
  /* Softdevice S140 reserves 156K flash in the beginning and 31K of RAM. */
  FLASH : ORIGIN = 0x0 + 156K, LENGTH = 976K - 156K - 256K
  DATA_FLASH : ORIGIN = 0x0 + 976K - 256K, LENGTH = 256K
  RAM : ORIGIN = 0x20000000 + 31K, LENGTH = 256K - 31K
}

SECTIONS {
  PROVIDE(_data_flash_start = ORIGIN(DATA_FLASH));
  PROVIDE(_data_flash_size = LENGTH(DATA_FLASH));
}
