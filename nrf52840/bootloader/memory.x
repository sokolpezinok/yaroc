MEMORY
{
  /* NOTE 1 K = 1 KiBi = 1024 bytes */
  /* NRF52840 with Softdevice S140 7.3.0 */
  /* Bootloader resides in the upper 48K of flash (leaving last 8K for MBR params/settings) */
  FLASH                             : ORIGIN = 0x000F4000, LENGTH = 40K
  BOOTLOADER_STATE                  : ORIGIN = 0x000B2000, LENGTH = 8K
  ACTIVE                            : ORIGIN = 0x00027000, LENGTH = 276K
  DFU                               : ORIGIN = 0x0006C000, LENGTH = 280K
  UICR_BOOTLOADER                   : ORIGIN = 0x10001014, LENGTH = 0x00000004
  /* First 8 bytes of RAM are reserved for the Nordic SoftDevice MBR interrupt forwarding */
  RAM                               : ORIGIN = 0x20000008, LENGTH = 256K - 8
}

__bootloader_start = ORIGIN(FLASH);
__bootloader_state_start = ORIGIN(BOOTLOADER_STATE);
__bootloader_state_end = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE);
__bootloader_active_start = ORIGIN(ACTIVE);
__bootloader_active_end = ORIGIN(ACTIVE) + LENGTH(ACTIVE);
__bootloader_dfu_start = ORIGIN(DFU);
__bootloader_dfu_end = ORIGIN(DFU) + LENGTH(DFU);

SECTIONS
{
  .uicr_bootloader_start_address :
  {
    KEEP(*(.uicr_bootloader_start_address))
  } > UICR_BOOTLOADER
}
