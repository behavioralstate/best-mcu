#!/usr/bin/env bash
# A self-signed certificate for trying the door on this machine: in QEMU, or on a board on the local network.
# It goes to firmware/certs, which git ignores; the key never leaves this machine. A board known by a real name
# gets a real certificate for that name instead.
#
#   scripts/dev-cert.sh [name]      default: localhost
set -euo pipefail
name="${1:-localhost}"
dir="$(cd "$(dirname "$0")/.." && pwd)/firmware/certs"
mkdir -p "$dir"
# A Windows openssl reads a Windows path: the subject below must not be converted, so the path is given as one.
out="$(cygpath -m "$dir" 2>/dev/null || echo "$dir")"
MSYS_NO_PATHCONV=1 openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 30 \
  -subj "/CN=$name" -addext "subjectAltName=DNS:$name" \
  -keyout "$out/key.pem" -out "$out/cert.pem" 2>/dev/null
# ESP-IDF reads a PEM up to its closing NUL.
printf '\0' >> "$dir/cert.pem"
printf '\0' >> "$dir/key.pem"
echo "made $dir/cert.pem and key.pem for $name, good for 30 days"
