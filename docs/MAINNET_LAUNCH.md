# Opolys Mainnet Launch Runbook

This runbook is the operator path for launching Opolys mainnet. It keeps the gold analogy honest: genesis supply is derived from real gold production data, attested once, verified by everyone, and then treated as consensus input.

Read `docs/THREAT_MODEL.md` before launch. It lists the remaining trust assumptions, including the single production genesis operator key and the operational rules around RPC auth and dry-run attestations. Read `docs/OPERATOR_CONFIG.md` for the flag-by-flag node, wallet, and ceremony configuration rules.

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

To build a checksummed local operator package, run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build_release.ps1
```

The package is written under `dist/` and includes the node, wallet, genesis
ceremony tools, launch docs, a release manifest, and SHA-256 checksums. See
`docs/RELEASE.md` for the verification flow.

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
7. Query `opl_getChainInfo`, `opl_getBlockByHeight`, `opl_getBlockAssayCertificate`, `opl_getMintLedger`, `opl assay`, and `opl ledger`.
8. Send one wallet transaction over loopback RPC with `opl --rpc-api-key ... send`.
9. Query `opl bond-minimum`, bond the local key as a refiner, and query `opl_getRefiners`, `opl_getRefinerHallmark`, plus `opl refiner`.
10. Restart again and confirm the block, transaction, balances, chain height, assay certificate, mint ledger, refiner hallmark, and wallet refiner view persist.

`--allow-solo-mining` bypasses the normal 3-outbound-peer mining quorum only
for an isolated rehearsal. Do not use it for production mainnet mining.

The repository also includes a repeatable local rehearsal script:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\launch_rehearsal.ps1
```

The script creates a dry-run ceremony, verifies it, starts an isolated mining
node, mines blocks, submits a wallet transfer over authenticated loopback RPC,
bonds the local key as a refiner using the live minimum reported by
`opl_getChainInfo` plus a small inclusion buffer, confirms the wallet
`bond-minimum` output agrees, records the refiner hallmark through both raw RPC
and `opl refiner`, records assay certificates through both raw RPC and
`opl assay`, records the Mint Ledger through both raw RPC and `opl ledger`,
checks the economic books against supply and block assay receipts, restarts the
node, and confirms the height, recipient balance, assay certificate, mint
ledger, economic invariants, and refiner stake persist. Its artifacts are written under
`launch-rehearsal-local/`, which is ignored by git.
The final human-readable summary is `launch-rehearsal-local/launch-report.md`.
If a transfer is submitted just after the miner has already assembled a block
candidate, inclusion may take one additional mined block. The rehearsal waits
for the recipient balance instead of assuming the next block must contain it.
The default mining wait is intentionally generous because local EVO-OMAP block
times vary; a slow run is not a failure unless the timeout is reached.

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

Before bonding a refiner, query the live dynamic minimum:

```bash
opl --rpc-url http://127.0.0.1:4171 bond-minimum
```

Bond above the minimum when blocks may arrive before inclusion, because the
minimum can rise as `total_issued` increases.

After bonding, check the refiner's status and recent hallmark activity:

```bash
opl --rpc-url http://127.0.0.1:4171 refiner <hex_object_id>
```

Inspect a block's assay certificate:

```bash
opl --rpc-url http://127.0.0.1:4171 assay <height_or_hash>
```

Inspect aggregate mint, burn, and refiner fee accounting:

```bash
opl --rpc-url http://127.0.0.1:4171 ledger
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

Block-producing nodes must include `--key-file`. Read-only observer nodes may
omit it, but `--mine`, `--refine`, and external mining jobs require a real
producer identity so rewards and refiner signatures bind to an on-chain account.

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
