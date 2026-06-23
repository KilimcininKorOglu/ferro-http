/* Memory layout for a generic Cortex-M4F part (e.g. STM32F4 class). The exact
   origins/lengths are placeholders for a self-contained link; a real board
   supplies its own regions. */
MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 256K
  RAM : ORIGIN = 0x20000000, LENGTH = 64K
}
