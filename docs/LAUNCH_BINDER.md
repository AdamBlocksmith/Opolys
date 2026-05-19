# Opolys Launch Binder

This binder is the launch-day operator path for Opolys mainnet. It assumes the
operator is using a packaged release artifact rather than building directly from
source.

Use it together with:

- `RELEASE.md` for package and checksum details
- `OPERATOR_CONFIG.md` for flag-by-flag rules
- `THREAT_MODEL.md` for residual risks and operator assumptions

## 1. Download The Release Package

Use the GitHub `Release Artifacts` workflow output or the tagged GitHub Release
asset for your platform:

- `opolys-windows-x86_64`
- `opolys-linux-x86_64`
- `opolys-macos-arm64`

Each artifact contains a packaged directory, a `.zip` archive, and the archive
`.zip.sha256` file.

## 2. Verify The Archive

Windows PowerShell:

```powershell
Get-FileHash -Algorithm SHA256 .\opolys-<version>-<commit>-<host>.zip
Get-Content .\opolys-<version>-<commit>-<host>.zip.sha256
```

Linux/macOS:

```bash
shasum -a 256 opolys-<version>-<commit>-<host>.zip
cat opolys-<version>-<commit>-<host>.zip.sha256
```

The two hashes must match before the archive is used.

## 3. Verify The Package Contents

Extract the archive, then verify every file against `SHA256SUMS.txt`.

Windows PowerShell:

```powershell
Get-ChildItem .\opolys-<version>-<commit>-<host> -File |
  Where-Object { $_.Name -ne "SHA256SUMS.txt" } |
  Sort-Object Name |
  ForEach-Object {
      $hash = (Get-FileHash -Algorithm SHA256 $_.FullName).Hash.ToLowerInvariant()
      "$hash  $($_.Name)"
  }
```

Linux/macOS:

```bash
cd opolys-<version>-<commit>-<host>
grep -v '  SHA256SUMS.txt$' SHA256SUMS.txt | shasum -a 256 -c -
```

The recomputed hashes must match the package's `SHA256SUMS.txt`.

## 4. Prepare Keys

Keep four roles separate:

- Genesis operator key: signs the one-time genesis ceremony attestation
- Miner key: earns mined block rewards
- Refiner key: bonds OPL and signs Proof-of-Refinement blocks
- Wallet mnemonic: controls user funds

Export a wallet-derived node key when a node needs to mine or refine:

```bash
opl export-key-file --from-stdin /secure/path/producer.key
```

Block-producing nodes must use `--key-file`. The node rejects `--mine`,
`--refine`, and external mining jobs without a real producer key.

## 5. Run The Genesis Ceremony

Run the ceremony from the extracted package directory.

Windows PowerShell:

```powershell
.\genesis-ceremony.exe `
  --operator "The Blocksmith" `
  --operator-key-file C:\secure\opolys-genesis-operator.key `
  --output-dir C:\secure\opolys-genesis-mainnet
```

Linux/macOS:

```bash
./genesis-ceremony \
  --operator "The Blocksmith" \
  --operator-key-file /secure/opolys-genesis-operator.key \
  --output-dir /secure/opolys-genesis-mainnet
```

Production ceremony rules:

- Prefer automatic source fetches.
- Use manual source entry only if automatic fetch/parse fails.
- Every manual value must include reproducible evidence notes.
- Do not use `--dry-run` for production data.
- Preserve the full ceremony output directory.

## 6. Verify The Ceremony

Windows PowerShell:

```powershell
.\genesis-ceremony.exe verify `
  --attestation C:\secure\opolys-genesis-mainnet\genesis_attestation.json
```

Linux/macOS:

```bash
./genesis-ceremony verify \
  --attestation /secure/opolys-genesis-mainnet/genesis_attestation.json
```

Expected result:

```text
RESULT: PASS
```

Do not launch if verification fails or prints unexpected warnings.

## 7. Start A Private Miner Node

This is the safer production shape for a miner/refiner machine: P2P enabled, RPC
disabled, persistent data directory, explicit producer key.

Windows PowerShell:

```powershell
.\opolys-node.exe `
  --genesis-params C:\secure\opolys-genesis-mainnet\genesis_attestation.json `
  --data-dir C:\opolys\data `
  --mine `
  --key-file C:\secure\producer.key `
  --no-rpc
```

Linux/macOS:

```bash
./opolys-node \
  --genesis-params /secure/opolys-genesis-mainnet/genesis_attestation.json \
  --data-dir /var/lib/opolys \
  --mine \
  --key-file /secure/producer.key \
  --no-rpc
```

Do not use `--allow-solo-mining` for normal mainnet operation. It exists for
isolated rehearsal, first-node bootstrapping, and diagnostics.

## 8. Start A Local RPC Node

Use loopback RPC when a local wallet or operator script needs to talk to the
node.

```bash
opolys-node \
  --genesis-params /secure/opolys-genesis-mainnet/genesis_attestation.json \
  --data-dir /var/lib/opolys \
  --mine \
  --key-file /secure/producer.key \
  --rpc-listen-addr 127.0.0.1 \
  --rpc-api-key "$OPOLYS_RPC_API_KEY"
```

Rules:

- Keep RPC on `127.0.0.1` unless a firewall or authenticated reverse proxy is
  protecting it.
- Do not use `--no-rpc-auth` on public RPC.
- If `--rpc-api-key` is omitted, the node generates a random key and prints it
  once at startup.

## 9. Check The Launch Summary

At startup, the node prints `Launch configuration summary`.

Confirm:

- `chain_id` is the expected mainnet chain id.
- `genesis_hash` matches the verified ceremony.
- `latest_hash` equals `genesis_hash` at height 0.
- `data_dir` is the intended persistent directory.
- `rpc_auth` is enabled unless RPC is intentionally disabled.
- `production_mode` matches the node role.
- `producer_id` is not zero for mining/refining nodes.
- `solo_mining` is false for normal mainnet operation.
- `dry_run_genesis_allowed` is false for production data.

Any warning about `--allow-dry-run-genesis`, `--allow-solo-mining`, or
`--no-rpc-auth` must be intentional rehearsal/private-lab behavior, not a normal
mainnet setting.

## 10. Run Wallet And RPC Sanity Checks

Read chain state:

```bash
opl --rpc-url http://127.0.0.1:4171 ledger
opl --rpc-url http://127.0.0.1:4171 bond-minimum
```

Check a wallet balance:

```bash
opl --rpc-url http://127.0.0.1:4171 balance <hex_object_id>
```

Check a block's assay certificate after blocks exist:

```bash
opl --rpc-url http://127.0.0.1:4171 assay <height_or_hash>
```

Check a refiner after bonding:

```bash
opl --rpc-url http://127.0.0.1:4171 refiner <hex_object_id>
```

## 11. Refiner Bonding

Query the live minimum immediately before bonding:

```bash
opl --rpc-url http://127.0.0.1:4171 bond-minimum
```

Bond above the minimum when blocks may arrive before inclusion, because the
minimum can rise as total issued supply increases.

Refiners start in `Bonding`, mature after one full epoch, then compete for
`Active` status by total stake. Active refiner selection is stake-weighted:

```text
chance_to_produce = refiner_total_stake / total_active_stake
```

Refiners receive no passive issuance and no time-based yield. In a
refiner-produced block, ordinary transaction fees are paid to the selected
refiner producer because that refiner moved the chain during miner silence. In a
mined block, ordinary transaction fees are burned.

## 12. Restart And Persistence Check

After the first successful startup:

1. Stop the node cleanly.
2. Restart with the same `--data-dir` and `--genesis-params`.
3. Confirm the same `genesis_hash`.
4. Confirm height, balances, refiner state, assay certificates, and mint ledger
   still load.

The node stores mainnet data under:

```text
<data-dir>/mainnet
```

Do not reuse a rehearsal data directory for production.

## 13. Stop Conditions

Stop the launch and diagnose before proceeding if:

- Ceremony verification does not return `RESULT: PASS`.
- Two machines compute different genesis hashes from the same attestation.
- A production node logs `dry_run_genesis_allowed=true`.
- A normal mainnet miner logs `solo_mining=true`.
- Public RPC is reachable without authentication.
- Restart changes genesis hash, latest hash, height, balances, or ledger data.
- Nodes using the same attestation disagree on chain height or accepted blocks.

## 14. Final Pre-Announcement Checklist

- Release artifact hash verified.
- Package file hashes verified.
- Genesis attestation verified independently.
- Genesis hash recorded.
- Operator, miner, refiner, and wallet keys separated.
- Persistent data directory selected and backed up as appropriate.
- RPC auth mode confirmed.
- No dry-run flags in production.
- No solo-mining flag in normal mainnet mode.
- At least two independent machines can start from the same attestation.
- Launch summary captured in operator notes.
