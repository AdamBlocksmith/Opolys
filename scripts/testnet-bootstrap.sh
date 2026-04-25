#!/bin/bash
# Opolys Testnet Bootstrap Script
#
# This script sets up a local testnet node with pre-funded genesis accounts.
# It generates a miner/validator key, initializes the data directory, and
# starts the node with mining and validation enabled.
#
# Usage:
#   ./scripts/testnet-bootstrap.sh          # Start with defaults
#   ./scripts/testnet-bootstrap.sh --reset   # Reset chain data and start fresh
#
# Prerequisites:
#   - Rust toolchain (cargo)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DATA_DIR="$PROJECT_DIR/testnet-data"
KEY_FILE="$DATA_DIR/miner.key"
PORT=4170
RPC_PORT=4171

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}╔════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║     Opolys Testnet Bootstrap Script    ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════╝${NC}"
echo ""

# Handle --reset flag
if [[ "${1:-}" == "--reset" ]]; then
    echo -e "${YELLOW}Resetting testnet data directory...${NC}"
    rm -rf "$DATA_DIR"
    echo -e "${GREEN}Testnet data directory cleared.${NC}"
    echo ""
fi

# Build the node binary if needed
echo -e "${YELLOW}Building opolys-node binary...${NC}"
cd "$PROJECT_DIR"
if ! cargo build --release -p opolys-node 2>/dev/null; then
    echo -e "${RED}Failed to build opolys-node binary. Make sure cargo is installed.${NC}"
    exit 1
fi
echo -e "${GREEN}Build complete.${NC}"
echo ""

# Create data directory
mkdir -p "$DATA_DIR"

# Generate miner/validator key if it doesn't exist
if [[ ! -f "$KEY_FILE" ]]; then
    echo -e "${YELLOW}Generating miner/validator key...${NC}"
    openssl rand -hex 32 > "$KEY_FILE"
    chmod 600 "$KEY_FILE"
    echo -e "${GREEN}Key generated at $KEY_FILE${NC}"
    SEED_HEX=$(cat "$KEY_FILE")
    echo -e "${YELLOW}Seed (hex): $SEED_HEX${NC}"
    echo -e "${YELLOW}IMPORTANT: Back up this key file! It controls your validator identity.${NC}"
    echo ""
else
    echo -e "${GREEN}Using existing key at $KEY_FILE${NC}"
    echo ""
fi

# Start the node
echo -e "${GREEN}Starting Opolys testnet node...${NC}"
echo -e "  Port:       $PORT"
echo -e "  RPC Port:   $RPC_PORT"
echo -e "  Data Dir:   $DATA_DIR"
echo -e "  Key File:   $KEY_FILE"
echo -e "  Mode:       TESTNET (3 pre-funded accounts × 10,000 OPL)"
echo -e "  Mining:     enabled"
echo -e "  Validating: enabled"
echo ""

cargo run --release -p opolys-node -- \
    --port "$PORT" \
    --rpc-port "$RPC_PORT" \
    --data-dir "$DATA_DIR" \
    --key-file "$KEY_FILE" \
    --mine \
    --validate \
    --testnet \
    --log-level info