# Opolys Mainnet Launch Runbook

This runbook is the operator path for launching Opolys mainnet. It keeps the gold analogy honest: genesis supply is derived from real gold production data, attested once, verified by everyone, and then treated as consensus input.

Read `docs/THREAT_MODEL.md` before launch. It lists the remaining trust assumptions, including the single production genesis operator key and the operational rules around RPC auth and dry-run attestations.

## 1. Build From A Clean Checkout

```bash
git clone https://github.com/AdamBlocksmith/Opolys.git
cd Opolys
cargo build --release
cargo test -p opolys-consensus
cargo test -p opolys-node
cargo test --manifest-path vendor/evo-omap/Cargo.toml test_known_answer --lib
cargo fmt --check
```

`evo-omap` is vendored at `vendor/evo-omap`. Operators do not download it separately; Cargo builds the audited source committed in this repository.

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
  --allow-dry-run-genesis \
  --no-rpc \
  --no-bootstrap
```

The node should create `./launch-rehearsal-data/mainnet` and stay running. The `--allow-dry-run-genesis` flag is intentionally required because the dry-run ceremony uses a public test signing key. Never use that flag for production mainnet data.

For the full launch rehearsal, use a fresh temporary data directory and complete this sequence:

1. Generate and verify a fresh dry-run attestation.
2. Start a node with `--allow-dry-run-genesis --no-bootstrap`.
3. Stop and restart the node against the same data directory.
4. Confirm restart loads the same genesis hash and current height.
5. Start a mining node with a throwaway miner key, local RPC, and `--allow-solo-mining`.
6. Mine at least one block.
7. Query `opl_getChainInfo` and `opl_getBlockByHeight`.
8. Send one wallet transaction over loopback RPC with `opl --rpc-api-key ... send`.
9. Restart again and confirm the block, transaction, balances, and chain height persist.

`--allow-solo-mining` bypasses the normal 3-outbound-peer mining quorum only
for an isolated rehearsal. Do not use it for production mainnet mining.

The repository also includes a repeatable local rehearsal script:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\launch_rehearsal.ps1
```

The script creates a dry-run ceremony, verifies it, starts an isolated mining
node, mines blocks, submits a wallet transfer over authenticated loopback RPC,
restarts the node, and confirms the height and recipient balance persist. Its
artifacts are written under `launch-rehearsal-local/`, which is ignored by git.

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
The wallet can export the same mnemonic-derived account to a node key file:

```bash
opl export-key-file --from-stdin /secure/path/miner.key
```

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
- Miners do not use `--allow-solo-mining` outside isolated rehearsal.
- Bootstrap peers are explicit and reachable.
- At least two independent machines can mine or verify blocks from the same genesis.
- The threat model has been reviewed, including residual risks that require testnet time or an external audit rather than another local patch.

## 6. What The Genesis Data Does

The ceremony data sets the base reward only. It does not make gold spot price a live oracle. After genesis, block rewards are driven by consensus state: base reward, difficulty, and vein yield. Spot price is captured in the attestation for transparency and historical anchoring, while annual mine production is the gold analogy that determines initial OPL emission.
