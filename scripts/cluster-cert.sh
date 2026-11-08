#!/usr/bin/env bash
# The board's real certificate: the chain and key cert-manager keeps in a Kubernetes secret (kubectl's current
# context), put in firmware/certs, which git ignores, without either being shown. Run again after each renewal, then
# build and flash the firmware: the certificate is built into it.
#
#   scripts/cluster-cert.sh [namespace] [secret]      default: behavioralstateprotocol robot-tls
set -euo pipefail
[ $# -le 2 ] || { echo "scripts/cluster-cert.sh: two arguments at most: scripts/cluster-cert.sh [namespace] [secret]"; exit 1; }
ns="${1-behavioralstateprotocol}"
secret="${2-robot-tls}"
dir="$(cd "$(dirname "$0")/.." && pwd)/firmware/certs"
mkdir -p "$dir"
refuse() { rm -f "$dir/cert.pem.new" "$dir/key.pem.new"; echo "scripts/cluster-cert.sh: $1"; exit 1; }
for part in crt key; do
  kubectl -n "$ns" get secret "$secret" -o "jsonpath={.data.tls\.$part}" 2>/dev/null | base64 -d > "$dir/$part.new" \
    && [ -s "$dir/$part.new" ] || { rm -f "$dir/crt.new" "$dir/key.new"; \
      refuse "no tls.$part in secret $ns/$secret: is kubectl on the cluster, and the certificate issued?"; }
done
mv "$dir/crt.new" "$dir/cert.pem.new"
mv "$dir/key.new" "$dir/key.pem.new"
[ "$(openssl x509 -in "$dir/cert.pem.new" -noout -pubkey | openssl md5)" = \
  "$(openssl pkey -in "$dir/key.pem.new" -pubout | openssl md5)" ] || refuse "the key in $ns/$secret does not match its certificate"
# ESP-IDF reads a PEM up to its closing NUL.
printf '\0' >> "$dir/cert.pem.new"
printf '\0' >> "$dir/key.pem.new"
mv "$dir/key.pem.new" "$dir/key.pem"
mv "$dir/cert.pem.new" "$dir/cert.pem"
openssl x509 -in "$dir/cert.pem" -noout -subject -enddate | tr '\n' ' '
echo "is in $dir with its key"
