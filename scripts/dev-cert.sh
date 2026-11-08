#!/usr/bin/env bash
# A self-signed certificate for trying the door on this machine: in QEMU, or on a board on the local network.
# It goes to firmware/certs, which git ignores; the key never leaves this machine. A board known by a real name
# gets a real certificate for that name instead.
#
#   scripts/dev-cert.sh [name|address]      default: localhost
#
# BEST_MCU_CERT_DIR  where the pair goes (default: firmware/certs, the board's; QEMU's is firmware/certs/qemu)
#
# A name is a hostname of 64 characters at most (labels of up to 63 letters, digits and inner hyphens, joined by
# dots); an address, any name of digits and dots, is IPv4: four numbers of 0 to 255 with no leading zero. The pair is
# replaced only when both are made.
set -euo pipefail
[ $# -le 1 ] || { echo "scripts/dev-cert.sh: one name or address at most: scripts/dev-cert.sh [name|address]"; exit 1; }
name="${1-localhost}"
refuse() { echo "scripts/dev-cert.sh: '$name' $1"; exit 1; }
if [[ "$name" =~ ^[0-9.]+$ ]]; then  # a board reached by its address is named by it
  ipv4="is no IPv4 address: four numbers of 0 to 255, with no leading zero, joined by dots"
  [[ "$name" =~ ^[0-9]+(\.[0-9]+){3}$ ]] || refuse "$ipv4"
  IFS=. read -r -a octets <<<"$name"
  for o in "${octets[@]}"; do
    [[ "$o" =~ ^(0|[1-9][0-9]{0,2})$ ]] && (( o <= 255 )) || refuse "$ipv4"
  done
  san="IP:$name"
else
  label='[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?'
  [[ ${#name} -le 64 && "$name" =~ ^$label(\.$label)*$ ]] \
    || refuse "is no hostname: 64 characters at most, labels of up to 63 letters, digits and inner hyphens, \
joined by dots"
  san="DNS:$name"
fi
dir="${BEST_MCU_CERT_DIR:-$(cd "$(dirname "$0")/.." && pwd)/firmware/certs}"
mkdir -p "$dir"
# A Windows openssl reads a Windows path: the subject below must not be converted, so the path is given as one.
out="$(cygpath -m "$dir" 2>/dev/null || echo "$dir")"
MSYS_NO_PATHCONV=1 openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 30 \
  -subj "/CN=$name" -addext "subjectAltName=$san" \
  -keyout "$out/key.pem.new" -out "$out/cert.pem.new" 2>/dev/null \
  || { rm -f "$dir/key.pem.new" "$dir/cert.pem.new"; refuse "was refused by openssl: no certificate was made"; }
# ESP-IDF reads a PEM up to its closing NUL.
printf '\0' >> "$dir/cert.pem.new"
printf '\0' >> "$dir/key.pem.new"
mv "$dir/key.pem.new" "$dir/key.pem"
mv "$dir/cert.pem.new" "$dir/cert.pem"
echo "made $dir/cert.pem and key.pem for $name, good for 30 days"
