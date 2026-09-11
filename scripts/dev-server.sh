#!/usr/bin/env bash
set -e

# Run UniDrop Go Server locally in development mode
cd "$(dirname "$0")/../server"

export UNIDROP_LISTEN_ADDR=":8080"
export UNIDROP_PSK_SECRET="dev-insecure-psk-secret"
export UNIDROP_STUN_ADDR=":3478"

echo "==> Starting UniDrop Go Relay Server on ${UNIDROP_LISTEN_ADDR}..."
go run ./cmd/unidrop-server
