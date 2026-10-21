#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
role="${1:-source}"
case "$role" in
  source) template=config.toml;;
  receiver|shared-receiver|shred-only|private-source|private-receiver) template="config.$role.toml";;
  *) echo "Usage: ./setup.sh source|receiver|shared-receiver|shred-only|private-source|private-receiver" >&2; exit 1;;
esac
# Source checkouts keep templates in the root; release packages put them in configs/.
if [[ -d configs ]]; then template="configs/$template"; fi
if [[ ! -e config.toml ]]; then cp "$template" config.toml
elif [[ "$template" != config.toml ]]; then
  echo "Keeping existing config.toml (to switch roles, back it up and copy $template)."
fi
case "$role" in receiver|shared-receiver|private-receiver)
  command -v openssl >/dev/null || { echo "openssl is required to generate receiver credentials" >&2; exit 1; }
  umask 077
  if [[ ! -e .bridge-token ]]; then openssl rand -hex 32 > .bridge-token; fi
  chmod 600 .bridge-token
  if [[ "$role" == private-receiver ]]; then
    echo "Copy .bridge-token securely to the source. Private templates use plaintext TCP and need no TLS certificate."
    echo "Existing config.toml is preserved; keep any credentials required by its TLS settings."
    exit 0
  fi
  mkdir -p certs
  if [[ ! -e certs/tunnel-cert.pem && ! -e certs/tunnel-key.pem ]]; then
    tls_name="${TLS_NAME:-bridge.example.com}"
    [[ "$tls_name" =~ ^[a-zA-Z0-9][a-zA-Z0-9.-]*$ ]] || { echo "TLS_NAME must be a DNS name" >&2; exit 1; }
    openssl req -x509 -newkey rsa:2048 -nodes -days 365 \
      -keyout certs/tunnel-key.pem -out certs/tunnel-cert.pem \
      -subj "/CN=$tls_name" -addext "subjectAltName=DNS:$tls_name" \
      -addext "basicConstraints=critical,CA:FALSE" 2>/dev/null
    echo "Created TLS certificate for $tls_name. Match source grpc.server_name to this name."
  elif [[ ! -f certs/tunnel-cert.pem || ! -f certs/tunnel-key.pem ]]; then
    echo "Incomplete existing cert/key pair; refusing to replace it" >&2; exit 1
  fi
  chmod 600 certs/tunnel-key.pem
  echo "Copy .bridge-token and certs/tunnel-cert.pem securely to the source. Keep tunnel-key.pem here."
  ;;
  source) echo "Copy the receiver's .bridge-token and certs/tunnel-cert.pem here before starting gRPC.";;
  private-source) echo "Copy the receiver's .bridge-token here. Private templates use plaintext TCP; existing TLS configurations still need their certificates.";;
esac
