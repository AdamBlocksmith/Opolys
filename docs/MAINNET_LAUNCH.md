# Opolys Mainnet Launch Runbook

This runbook is the operator path for launching Opolys mainnet. It keeps the gold analogy honest: genesis supply is derived from real gold production data, attested once, verified by everyone, and then treated as consensus input.

## 1. Build From A Clean Checkout

```bash
git clone https://github.com/AdamBlocksmith/Opolys.git
cd Opolys
cargo build --release
cargo test -p opolys-consensus
cargo test -p opolys-node
cargo fmt --check
```

`evo-omap` is a pinned Git dependency in `Cargo.toml`. Operators do not download it separately; Cargo fetches the exact audited revision.

## 2. Rehearse The Ceremony

Run this before launch day. It performs no network fetches and uses a deterministic test key.

```bash
cargo run -p genesis-ceremony -- \
  --dry-run \
  --output-dir ./genesis-dry-run

cargo run -p genesis-ceremony -- verify \
  --attestation ./genesis-dry-run/genesis_attestation.json
```

Expected result:

```text
RESULT: PASS
```

Smoke-start a node against the dry-run attestation:

```bash
cargo run -p opolys-node -- \
  --genesis-params ./genesis-dry-run/genesis_attestation.json \
  --data-dir ./launch-rehearsal-data \
  --no-rpc \
  --no-bootstrap
```

The node should create `./launch-rehearsal-data/mainnet` and stay running.

## 3. Production Ceremony

Use a dedicated operator key file and keep it offline/backed up. The key file contains the ed25519 seed that signs the genesis attestation.

```bash
cargo run --release -p genesis-ceremony -- \
  --operator "The Blocksmith" \
  --operator-key-file /secure/path/operator_signing_key.txt \
  --output-dir ./genesis-mainnet
```

Production ceremony rules:

- The automatic source fetch is preferred.
- Manual source entry is only a fallback when a source cannot be fetched or parsed.
- Every manual entry must include evidence notes good enough for another operator to reproduce.
- The ceremony aborts if fewer than the required production sources succeed.
- The ceremony must finish inside the five-minute window.

Verify before using the attestation:

```bash
cargo run --release -p genesis-ceremony -- verify \
  --attestation ./genesis-mainnet/genesis_attestation.json
```

Only launch if verification returns `RESULT: PASS` with no unexpected warnings.

## 4. Start Mainnet Nodes

Refiner/miner key files are 32-byte ed25519 seeds. Keep them separate from the genesis operator signing key.

Private node, no RPC:

```bash
cargo run --release -p opolys-node -- \
  --genesis-params ./genesis-mainnet/genesis_attestation.json \
  --data-dir ./data \
  --mine \
  --key-file /secure/path/miner.key \
  --no-rpc
```

Local RPC only, with an explicit API key:

```bash
cargo run --release -p opolys-node -- \
  --genesis-params ./genesis-mainnet/genesis_attestation.json \
  --data-dir ./data \
  --mine \
  --key-file /secure/path/miner.key \
  --rpc-listen-addr 127.0.0.1 \
  --rpc-api-key "$OPOLYS_RPC_API_KEY"
```

Public RPC should only be exposed behind an authenticated reverse proxy or firewall. Avoid `--no-rpc-auth` on mainnet unless another layer enforces authentication.

## 5. Launch Checks

Before announcing the network:

- Every seed and miner node uses the same `genesis_attestation.json`.
- `genesis-ceremony verify` passes independently on another machine.
- Nodes start from empty data directories and create `mainnet` storage.
- RPC write/mining methods require an API key unless RPC is disabled.
- Bootstrap peers are explicit and reachable.
- At least two independent machines can mine or verify blocks from the same genesis.

## 6. What The Genesis Data Does

The ceremony data sets the base reward only. It does not make gold spot price a live oracle. After genesis, block rewards are driven by consensus state: base reward, difficulty, and vein yield. Spot price is captured in the attestation for transparency and historical anchoring, while annual mine production is the gold analogy that determines initial OPL emission.
