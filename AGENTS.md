# AGENTS.md — Instructions for AI Assistants

## Auto-Push Rule

**After every meaningful change, commit and push to GitHub automatically.** Do not ask the user if you should commit — just do it. The user should never have to remind you about this.

Steps after completing a task:
1. `git add -A`
2. `git commit -m "<descriptive message>"`
3. `git push origin main`

If push fails due to conflicts, pull first then push.

## Code Style

- Language: Rust, edition 2024
- Comments are important — document all public items with `///` doc comments
- Module-level docs use `//!`
- Include inline comments on non-obvious logic
- No emoji in code or commits
- No `unwrap()` in production code — use `?`, `map_err`, or explicit error handling
- Use `FlakeAmount` (u64) for all monetary values, never floating point

## Testing

- Run `cargo test` before every commit
- All tests must pass before pushing
- Use `tempfile::tempdir()` for node tests that touch RocksDB — never use shared `./data`

## Project Conventions

### Naming
- OPL ($OPL) — the coin
- Flake — smallest unit (1/1,000,000 OPL)
- Pennyweight (dwt) — 0.01 OPL
- Grain (gr) — 0.0001 OPL
- Blake3-256 — the hash function, 32 bytes, everywhere
- ObjectId — Blake3-256(ed25519_pubkey), not the pubkey itself

### Architecture Decisions
- No tokens, no assets, no governance, no hardcoded fees
- Fees are market-driven and burned (not collected by validators)
- Only double-signing gets slashed
- Natural equilibrium model — no hard cap, difficulty and rewards emerge from chain state
- BASE_REWARD (440 OPL) is derived from real gold production data

### Dependency Versions
- libp2p 0.54
- ed25519-dalek 2.1
- pqc_dilithium 0.2
- bip39 2.2 (with `rand` feature)
- blake3 1.8
- RocksDB via `rocksdb` 0.22
- Borsh 1.5 for serialization
- axum 0.8 for RPC

## Commit Messages

Write descriptive commit messages that explain the "why" not just the "what". Do not mention the word "milestone" in commits.

## Current Status

Phase 3 (RPC + TX Lifecycle) is complete. Next phases in order:
1. **Phase 7a**: Security hardening (code audit, fuzz, overflow checks) — before genesis
2. **Phase 6**: Genesis ceremony — lock in real data on hardened code
3. **Phase 2**: Networking (P2P libp2p)
4. **Phase 4**: Staking & PoS transition
5. **Phase 5**: Wallet CLI
6. **Phase 7b**: Security audit (2nd pass — user-facing attack surface)
7. **Phase 8**: Testnet
8. **Phase 9**: Mainnet