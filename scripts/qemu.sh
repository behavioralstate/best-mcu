#!/usr/bin/env bash
# Builds the firmware for Espressif's QEMU and runs it there: the door on https://localhost:8443, its serial log on
# the terminal; Ctrl-C ends it. A certificate is made first when there is none (scripts/dev-cert.sh).
#
# What the emulator needs, and why:
# - only the components the door uses: esp_adc calibrates the chip's ADC at start, which QEMU does not emulate, and
#   would wait for it forever; esp-idf-svc needs driver, spiffs and fatfs present besides;
# - an eFuse image saying chip revision 0.3, the timer group's watchdog off and the PSRAM octal, as `idf.py qemu`
#   runs it;
# - its emulated Ethernet (the `qemu` feature), since it has no Wi-Fi.
#
# BEST_MCU_QEMU_PORT     the port on 127.0.0.1 the door is forwarded to (default 8443)
# BEST_MCU_QEMU_SERIAL   a file for the serial log, in place of the terminal
# BEST_MCU_QEMU_TARGET   where it builds (default target/qemu), apart from the board's build so neither redoes the other
# ESP_IDF_TOOLS_INSTALL_DIR  where ESP-IDF and its tools are, `custom:<dir>` (default: firmware/.embuild/espressif)
# QEMU is ESP-IDF's own build; it is installed with ESP-IDF's tools when missing.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
source "$here/scripts/env.sh"
port="${BEST_MCU_QEMU_PORT:-8443}"
target="${BEST_MCU_QEMU_TARGET:-$here/target/qemu}"
export CARGO_TARGET_DIR="$target"
# The manifest names the address the door is reached at.
export BEST_MCU_ENDPOINT="https://localhost:$port"
export ESP_IDF_COMPONENTS="main;driver;spiffs;fatfs;esp_https_server;esp_http_server;esp_eth;esp_netif;lwip;mbedtls;nvs_flash;pthread;esp_psram;vfs;esp_timer;log;freertos"
win() { cygpath -m "$1" 2>/dev/null || echo "$1"; }

[ -f "$here/firmware/certs/cert.pem" ] || "$here/scripts/dev-cert.sh"
(cd "$here/firmware" && cargo build --release --features qemu)
elf="$target/xtensa-esp32s3-espidf/release/best-mcu"
espflash save-image --chip esp32s3 --flash-size 16mb --merge "$elf" "$target/flash.bin" >/dev/null

# Chip revision 0.3: byte 38 of the 1024-byte eFuse block, as ESP-IDF's qemu_ext.py writes it.
{ head -c 38 /dev/zero; printf '\x0c'; head -c 985 /dev/zero; } > "$target/qemu_efuse.bin"

# ESP-IDF's tools, where scripts/env.sh has esp-idf-sys put them.
tools="${ESP_IDF_TOOLS_INSTALL_DIR#custom:}"
find_qemu() { ls "$tools"/tools/qemu-xtensa/*/qemu/bin/qemu-system-xtensa{,.exe} 2>/dev/null | head -1; }
qemu="$(find_qemu)"
if [ -z "$qemu" ]; then
  idf="$(ls -d "$tools"/esp-idf/v* | tail -1)"
  py="$(ls "$tools"/python_env/*/bin/python "$tools"/python_env/*/Scripts/python.exe 2>/dev/null | head -1)"
  IDF_TOOLS_PATH="$(win "$tools")" IDF_PATH="$(win "$idf")" "$py" "$(win "$idf/tools/idf_tools.py")" install qemu-xtensa
  qemu="$(find_qemu)"
fi

serial="mon:stdio"
[ -n "${BEST_MCU_QEMU_SERIAL:-}" ] && serial="file:$(win "$BEST_MCU_QEMU_SERIAL")"
exec "$qemu" -nographic -M esp32s3 -m 8M \
  -drive "file=$(win "$target/flash.bin"),if=mtd,format=raw" \
  -drive "file=$(win "$target/qemu_efuse.bin"),if=none,format=raw,id=efuse" \
  -global driver=nvram.esp32s3.efuse,property=drive,value=efuse \
  -global driver=timer.esp32s3.timg,property=wdt_disable,value=true \
  -global driver=ssi_psram,property=is_octal,value=true \
  -nic "user,model=open_eth,hostfwd=tcp:127.0.0.1:$port-:443" \
  -serial "$serial" -monitor none
