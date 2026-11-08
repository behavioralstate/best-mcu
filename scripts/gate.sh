#!/usr/bin/env bash
# The local proof of SDLC.md section 3: fmt, clippy with warnings as errors and the tests of the door; the board's
# firmware built (with Wi-Fi, a placeholder network, never run); then the qemu lane: the firmware built for Espressif's
# QEMU and run there, best-validate against the door it serves over HTTPS, HEAD answered as GET without the body,
# and the manifest sent whole to a client that holds it. The last line is GATE GREEN or GATE RED, and nothing else is
# a pass. CI runs this same script.
#
#   scripts/gate.sh [fmt|clippy|test|board|qemu|all]      default: all
#
# Needs cargo with the esp toolchain (espup), espflash, ldproxy, node (npx) and curl; on Windows, Git Bash with the
# MSVC Build Tools environment loaded. ESP_IDF_TOOLS_INSTALL_DIR is honoured where set, CARGO_TARGET_DIR for every
# build but QEMU's, which builds in BEST_MCU_QEMU_TARGET (scripts/qemu.sh). A certificate is made first when there is
# none (scripts/dev-cert.sh). The emulated door listens on 127.0.0.1:8447, away from scripts/qemu.sh's 8443, and is
# stopped at the end, Ctrl-C included.
set -uo pipefail
BEST_VALIDATE_VERSION=0.4.3

cd "$(dirname "$0")/.." && source scripts/env.sh || { echo "GATE RED"; exit 1; }
what="${1:-all}"
case "$what" in
  fmt|clippy|test|board|qemu|all) ;;
  *) echo "scripts/gate.sh: unknown step '$what'; one of fmt, clippy, test, board, qemu, all"; echo "GATE RED"; exit 1 ;;
esac
failed=0
step() {  # name command...: runs the command, keeps the verdict, never stops the script.
  local name="$1"; shift
  echo "=== $name"
  "$@"
  local rc=$?
  if [ "$rc" -ne 0 ]; then echo "FAILED: $name"; failed=1; fi
}
wants() { [[ "$what" == all || "$what" == "$1" ]]; }

fmt() { cargo fmt --all --check && (cd firmware && cargo fmt --check); }
cert() { [ -f firmware/certs/cert.pem ] || scripts/dev-cert.sh; }  # the firmware embeds one, and git has none
board() {  # The board's firmware, Wi-Fi included, built against a placeholder network: it is never flashed.
  cert && (cd firmware && BEST_MCU_WIFI_SSID=gate BEST_MCU_WIFI_PASSWORD=placeholder cargo build --release)
}
pid=""
stop() { [ -n "$pid" ] && kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null; pid=""; }  # QEMU, or its build
trap stop EXIT
trap 'stop; echo "GATE RED"; exit 1' INT TERM
qemu_lane() {
  local log port=8447 ok=1
  log="$(mktemp)"
  BEST_MCU_QEMU_PORT=$port BEST_MCU_QEMU_SERIAL="$log" scripts/qemu.sh &
  pid=$!
  # The build comes first, and the first one fetches and builds ESP-IDF: up to thirty minutes.
  for _ in $(seq 1 900); do
    kill -0 "$pid" 2>/dev/null || { echo "scripts/qemu.sh ended before the door was served"; ok=0; break; }
    grep -a -q "the door is served" "$log" 2>/dev/null && break
    grep -a -q -E "abort\(\)|Guru Meditation|panicked" "$log" 2>/dev/null && { echo "the firmware failed in QEMU:"; ok=0; break; }
    sleep 2
  done
  grep -a -q "the door is served" "$log" 2>/dev/null || { [ "$ok" = 1 ] && echo "the door was not served in QEMU"; ok=0; }
  if [ "$ok" = 1 ]; then
    # The certificate is the self-signed one of scripts/dev-cert.sh, accepted for this address alone.
    NODE_TLS_REJECT_UNAUTHORIZED=0 npx -y "@behavioralstate/best-validate@$BEST_VALIDATE_VERSION" \
      "https://localhost:$port" 2>&1 | grep -v NODE_TLS_REJECT_UNAUTHORIZED | tee "$log.validate"
    grep -q "^CONFORMANT" "$log.validate" || ok=0
    # A client holding the manifest is sent it whole, never a 304, which ESP-IDF would give a length of 0.
    local url="https://localhost:$port/.well-known/best" etag status
    etag="$(curl -sk -D - -o /dev/null "$url" | tr -d '\r' | sed -n 's/^[Ee][Tt][Aa][Gg]: //p')"
    status="$(curl -sk -o /dev/null -w '%{http_code}' -H "If-None-Match: $etag" "$url")"
    if [ -z "$etag" ] || [ "$status" != 200 ]; then
      echo "a client holding the manifest (If-None-Match: $etag) was answered $status, not the manifest whole"
      ok=0
    fi
    # HEAD is answered as GET, without the body: two on one connection both read as answers, which a body sent after
    # the first would break.
    local heads
    heads="$(curl -sk --http1.1 -I "$url" "$url" | tr -d '\r')"
    if [ "$(grep -c '^HTTP/1.1 200' <<<"$heads")" != 2 ] || [ "$(grep -ci '^etag: ' <<<"$heads")" != 2 ]; then
      echo "two HEADs on the manifest on one connection were not both answered 200 with the ETag:"
      echo "$heads"
      ok=0
    fi
  else
    tail -20 "$log"
  fi
  stop
  rm -f "$log" "$log.validate"
  [ "$ok" = 1 ]
}

if wants fmt; then step "fmt" fmt; fi
if wants clippy; then step "clippy" cargo clippy --workspace --all-targets -- -D warnings; fi
if wants test; then step "test" cargo test --workspace; fi
if wants board; then step "board firmware build" board; fi
if wants qemu; then step "qemu: the door served from the emulated ESP32-S3, best-validate over HTTPS" qemu_lane; fi

if [ "$failed" = 0 ]; then echo "GATE GREEN"; else echo "GATE RED"; exit 1; fi
