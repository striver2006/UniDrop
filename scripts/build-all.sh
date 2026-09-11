#!/usr/bin/env bash
set -e

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# Ensure cargo is in PATH
if [ -f "$HOME/.cargo/env" ]; then
    . "$HOME/.cargo/env"
fi

echo "==> [1/3] Building UniDrop Server (Go)..."
cd "$ROOT_DIR/server"
go test -race ./internal/...
go build -o bin/unidrop-server ./cmd/unidrop-server
echo "Server binary built: server/bin/unidrop-server"

echo "==> [2/3] Building UniDrop Client Frontend (Vite/React)..."
cd "$ROOT_DIR/client"
pnpm install
pnpm build
echo "Client UI assets built: client/dist/"

echo "==> [3/3] Checking UniDrop Client Core (Rust)..."
cd "$ROOT_DIR/client/src-tauri"
cargo check
cargo test
echo "Client Rust backend checks and tests passed!"

echo "==> All builds, tests, and checks completed successfully!"
