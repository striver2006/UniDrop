#!/usr/bin/env bash
set -e

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

echo "==> [1/2] Building UniDrop Server (Go)..."
cd "$ROOT_DIR/server"
go test -race ./internal/...
go build -o bin/unidrop-server ./cmd/unidrop-server
echo "Server binary built: server/bin/unidrop-server"

echo "==> [2/2] Building UniDrop Client Frontend (Vite/React)..."
cd "$ROOT_DIR/client"
pnpm install
pnpm build
echo "Client UI assets built: client/dist/"

echo "==> All builds completed successfully!"
