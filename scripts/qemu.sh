#!/usr/bin/env bash
# Builds the firmware for Espressif's QEMU and runs it there, the door on https://localhost:8443, its serial log on
# the terminal; Ctrl-C ends it. Run scripts/dev-cert.sh once first.
#
# What the emulator needs, and why:
# - only the components the door uses: esp_adc calibrates the chip's ADC at start, which QEMU does not emulate, and
#   would wait for it forever; esp-idf-svc needs driver, spiffs and fatfs present besides;
# - an eFuse image saying chip revision 0.3, the timer group's watchdog off and the PSRAM octal, as `idf.py qemu`
#   runs it;
# - its emulated Ethernet (the `qemu` feature), since it has no Wi-Fi.
#
# The build goes to $BEST_MCU_QEMU_TARGET (default target/qemu), apart from the board's, so neither rebuilds the other.
# QEMU is ESP-IDF's: `idf_tools.py install qemu-xtensa`, found under $IDF_TOOLS_PATH or on PATH.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
target="${BEST_MCU_QEMU_TARGET:-$here/target/qemu}"
export CARGO_TARGET_DIR="$target"
export ESP_IDF_COMPONENTS="main;driver;spiffs;fatfs;esp_https_server;esp_http_server;esp_eth;esp_netif;lwip;mbedtls;nvs_flash;pthread;esp_psram;vfs;esp_timer;log;freertos"

(cd "$here/firmware" && cargo build --release --features qemu)
elf="$target/xtensa-esp32s3-espidf/release/best-mcu"
espflash save-image --chip esp32s3 --flash-size 16mb --merge "$elf" "$target/flash.bin"

# Chip revision 0.3: byte 38 of the eFuse block, as ESP-IDF's qemu_ext.py writes it.
python - "$target/qemu_efuse.bin" <<'EOF'
import sys
efuse = bytearray(1024)
efuse[38] = 0x0c
open(sys.argv[1], "wb").write(efuse)
EOF

qemu="$(command -v qemu-system-xtensa || ls "${IDF_TOOLS_PATH:-$HOME/.espressif}"/tools/qemu-xtensa/*/qemu/bin/qemu-system-xtensa* 2>/dev/null | head -1)"
[ -n "$qemu" ] || { echo "no qemu-system-xtensa: install it with idf_tools.py install qemu-xtensa"; exit 1; }
win() { cygpath -m "$1" 2>/dev/null || echo "$1"; }
exec "$qemu" -nographic -M esp32s3 -m 8M \
  -drive "file=$(win "$target/flash.bin"),if=mtd,format=raw" \
  -drive "file=$(win "$target/qemu_efuse.bin"),if=none,format=raw,id=efuse" \
  -global driver=nvram.esp32s3.efuse,property=drive,value=efuse \
  -global driver=timer.esp32s3.timg,property=wdt_disable,value=true \
  -global driver=ssi_psram,property=is_octal,value=true \
  -nic user,model=open_eth,hostfwd=tcp:127.0.0.1:8443-:443 \
  -serial mon:stdio
