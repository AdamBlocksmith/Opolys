# Opolys Threat Model

This document names what Opolys is trying to protect, what is currently enforced in code, and what remains operationally risky before mainnet.

## Security Goals

Opolys protects one asset, OPL. The main goals are:

- Honest nodes agree on the same genesis block, block history, state root, balances, refiner state, and finalized height.
- Blocks cannot be relayed with altered state-mutating body data.
- Rewards, burns, staking, unbonding, and fees either apply completely or fail closed.
- Mainnet nodes do not accidentally join a dry-run or alternate genesis.
- Wallet keys and RPC controls are not exposed by default.
- EVO-OMAP proof-of-work is built from the vendored source in this repository, not from a moving git dependency.

## In Scope

The hardened code paths cover:

- Genesis ceremony verification, including canonical master hash, operator signature, pinned production operator public key, sanity bounds, and explicit `--allow-dry-run-genesis` for dry-run attestations.
- Block validation, including PoW, parent linkage, state-mutating body roots, timestamp bounds, transaction root, state root, and refiner producer checks.
- Emission math with deterministic integer arithmetic only on consensus paths.
- Difficulty retargeting from the rolling timestamp window.
- Refiner finality indexing without historical block scans during `apply_block`.
- RocksDB persistence with checksummed values, atomic writes, and storage schema version guard.
- JSON-RPC write/mining authentication by default, body limits, CORS allow-listing, and rate limiting.
- Wallet key files with restrictive permissions on Unix and Windows.
- Dependency security gates through cargo-audit and cargo-deny.

## Out Of Scope

Opolys does not claim to solve:

- A majority-hashpower attack. Like Bitcoin-style PoW, enough mining power can reorder recent blocks.
- Compromise of the production genesis operator key before launch. Current code limits the damage with sanity bounds, but a single operator key remains a trust point.
- A fully hostile operating system or wallet host.
- Social engineering of operators.
- DNS or hosting compromise of public websites and documentation.
- Exchange, custody, bridge, explorer, or third-party wallet security.

## Main Trust Assumptions

- Operators build from a clean checkout and committed `Cargo.lock`.
- All mainnet nodes use the same verified `genesis_attestation.json`.
- The production operator key is generated, stored, and used securely.
- Public RPC endpoints sit behind their own authentication, firewall, or reverse proxy.
- Bootstrap peer identities and addresses are distributed through more than one channel.
- Launch happens only after a fresh dry run and a public adversarial test period.

## Known Residual Risks

- Genesis still uses a single production operator signature. A future k-of-n ceremony would reduce this trust concentration, but the current launch path assumes one carefully protected operator key.
- Difficulty can still move according to system timestamps and hashpower. Timestamp compression is bounded by validation, but real launch hashrate should be observed in rehearsal.
- State roots are currently full-store hashes rather than an incremental Merkle state tree. This is safe for correctness but not the long-term scaling shape.
- The project still needs external audit and public testnet time before value-bearing launch.

## Operator Rules

- Never use `--allow-dry-run-genesis` with production data.
- Never use `--no-rpc-auth` on a public interface unless another layer authenticates requests.
- Keep the genesis operator key separate from miner/refiner keys.
- Verify the genesis attestation on a second machine before launch.
- Treat `Cargo.lock`, `rust-toolchain.toml`, `vendor/evo-omap`, and CI security gates as part of the consensus perimeter.
