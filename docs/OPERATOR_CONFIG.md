# Opolys Operator Configuration

This is the operator-facing configuration audit for Opolys nodes and launch tools. It lists the flags that change runtime behavior, what they are for, and the mainnet rule for each one.

## Node Identity

| Flag | Default | Mainnet rule |
|---|---:|---|
| `--genesis-params <path>` | required | Always required. Every node must use the same verified `genesis_attestation.json`. |
| `--data-dir <path>` | `./data` | Use a dedicated persistent directory. Opolys stores mainnet data under `<data-dir>/mainnet`. |
| `--key-file <path>` | none | Required for `--mine` or `--refine`. Export with `opl export-key-file`; do not reuse the genesis operator key. |
| `--log-level <level>` | `info` | Use `info` for normal operation; use `debug` only when diagnosing a problem. |

Block-producing nodes now fail at startup if `--mine` or `--refine` is used without `--key-file`. Read-only nodes may run without a key file.

## Networking

| Flag | Default | Mainnet rule |
|---|---:|---|
| `--port <port>` | `4170` | P2P UDP/QUIC listen port. If changed, the default RPC port also moves to `port + 1` unless `--rpc-port` is set. |
| `--bootstrap <multiaddr>` | none | Add explicit trusted bootstrap peers. Can be repeated or comma-separated. |
| `--no-bootstrap` | disabled | Rehearsal/private-network only unless the operator supplies trusted explicit peers. |

When `--no-bootstrap` is absent, the node uses the peer cache, DNS seeds, and any explicit `--bootstrap` peers. When `--no-bootstrap` is present, only explicit `--bootstrap` peers are dialed.

## RPC

| Flag | Default | Mainnet rule |
|---|---:|---|
| `--no-rpc` | disabled | Best choice for private miner/refiner nodes that do not need local wallet access. |
| `--rpc-port <port>` | `--port + 1` | Default is `4171` when the P2P port is `4170`. |
| `--rpc-listen-addr <ip>` | `127.0.0.1` | Keep loopback unless protected by firewall or authenticated reverse proxy. |
| `--rpc-api-key <key>` | random generated key | Set explicitly for stable authenticated write/mining RPC across restarts. |
| `--no-rpc-auth` | disabled | Do not use on a public interface. Only acceptable behind another authentication layer or in a private lab. |

Read-only RPC methods are public. Write and mining RPC methods require the API key unless `--no-rpc-auth` is explicitly set. If no API key is provided, the node generates a random one and prints it once at startup.

## Production Modes

| Flag | Default | Mainnet rule |
|---|---:|---|
| `--mine` | disabled | Enables local EVO-OMAP mining. Requires `--key-file`. |
| `--allow-solo-mining` | disabled | Rehearsal/private-lab only. Mainnet miners should wait for the peer quorum. |
| `--refine` | disabled | Enables Proof of Refinement block production when this key is the selected active refiner. Requires `--key-file`. |
| `--allow-dry-run-genesis` | disabled | Rehearsal only. Never use with production ceremony data. |

`--mine` and `--refine` may both be enabled on the same node. Mining is the primary production path; refinement only moves the chain when miners are silent and the local refiner is selected.

## Genesis Ceremony

| Flag | Default | Mainnet rule |
|---|---:|---|
| `--operator <name>` | `The Blocksmith` | Recorded in the attestation. Use the real ceremony operator name. |
| `--operator-key-file <path>` | generated in output dir | Use a dedicated ceremony key stored separately from miner/refiner keys. |
| `--output-dir <path>` | current directory | Use a fresh ceremony directory and preserve all outputs. |
| `--production-year <year>` | previous calendar year | Only override when the latest accepted annual source data is for a different year. |
| `--manual` | disabled | Fallback only. Every manual value must include evidence notes. |
| `--dry-run` | disabled | Rehearsal only. Uses a public deterministic test key. |
| `verify --attestation <path>` | n/a | Must return `RESULT: PASS` before launch. |

## Wallet

| Flag / command | Mainnet rule |
|---|---|
| `opl new` | Generates a 24-word mnemonic. Store it offline. |
| `opl --rpc-url <url>` | Defaults to loopback. Remote plain HTTP is rejected unless loopback; use HTTPS for remote RPC. |
| `opl --rpc-api-key <key>` | Required for write RPC such as `send` unless the node has disabled RPC auth. |
| `opl export-key-file --from-stdin <path>` | Preferred way to create a node-compatible miner/refiner key file. |
| `opl bond-minimum` | Query immediately before bonding; the minimum is dynamic. |
| `opl refiner <address>` | Check status and Hallmark activity after bonding. |
| `opl assay <height_or_hash>` | Inspect a block's economic receipt. |
| `opl ledger` | Inspect aggregate mint, burn, and refiner fee accounting. |

## Launch Rules

- Build from a clean checkout with committed `Cargo.lock`.
- Verify the genesis attestation independently before using it.
- Keep ceremony, miner, refiner, and wallet secrets separated.
- Do not use dry-run flags with production ceremony data.
- Do not mine mainnet with `--allow-solo-mining`.
- Do not expose unauthenticated write/mining RPC.
- Package binaries with `scripts/build_release.ps1` and verify checksums before copying them to another machine.
