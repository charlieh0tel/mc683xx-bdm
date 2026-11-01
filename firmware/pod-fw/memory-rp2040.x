/* Pico (RP2040): 2 MB flash after the 256-byte boot2 (embassy-rp's
   link-rp.x places it), 256 KB of striped SRAM (RP2040 datasheet 2.6). */
MEMORY {
    BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100
    FLASH : ORIGIN = 0x10000100, LENGTH = 2048K - 0x100
    RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}
