# Opolys ($OPL)

**Decentralized digital gold.** A pure coin blockchain with no tokens, no assets, no governance, and no hardcoded fees. Every parameter emerges from mathematics or market forces.

---

## Quick Start

### Prerequisites

- **Rust** 1.85+ (edition 2024)
- **RocksDB** system library (or builds from source via `librocksdb-sys`)
- **Clang/LLVM** (required by `libp2p` build)

### Build

```bash
git clone https://github.com/AdamBlocksmith/Opolys.git
cd Opolys
cargo build --release
```

### Run a Node

```bash
# Start with default settings (port 4170, RPC on 4171)
cargo run --release

# Custom ports and data directory
cargo run --release -- --port 5000 --rpc-port 5001 --data-dir /path/to/data

# Connect to a bootstrap peer
cargo run --release -- --bootstrap /ip4/1.2.3.4/tcp/4170

# Adjust log level
cargo run --release -- --log-level debug
```

### Run Tests

```bash
# All tests
cargo test

# Specific crate
cargo test -p opolys-core
cargo test -p opolys-consensus

# With output
cargo test -- --nocapture
```

---

## Philosophy

| Principle | Detail |
|---|---|
| **No hard cap** | Supply grows via block rewards; fees are burned, modeling real gold attrition |
| **No governance** | No on-chain governance, no voting, no committees |
| **No schedules** | Difficulty and rewards emerge from chain state, not from a calendar |
| **No hardcoded fees** | Fees are market-driven and burned entirely — validators earn from block rewards |
| **Only double-signing slashed** | No reversal windows, no confiscation for any other reason |
| **Gold-derived emission** | BASE_REWARD = 440 OPL, derived from annual gold production (~3,630 tonnes) |

---

## Currency Units

OPL uses 6 decimal places, named after real gold weight units:

| Unit | Flakes | OPL | Example |
|---|---|---|---|
| **OPL** | 1,000,000 | 1 | `1.000000 OPL` |
| **Flake** | 1 | 0.000001 | `0.000001 OPL` |

The only sub-unit is the **Flake** (1/1,000,000 OPL). All internal arithmetic uses `FlakeAmount` (u64) — no floating point anywhere.

---

## Architecture

```
Opolys
├── crates/
│   ├── core/        — Shared types, constants, error types
│   ├── crypto/      — Blake3-256, ed25519
│   ├── consensus/  — PoW, PoS, difficulty, emission, mempool, genesis
│   ├── execution/   — Transaction dispatcher (Transfer, Bond, Unbond)
│   ├── storage/     — RocksDB persistence layer
│   ├── networking/  — libp2p gossip/sync/discovery (scaffold)
│   ├── wallet/      — BIP-39, SLIP-0010, TxSigner
│   ├── rpc/         — Axum JSON-RPC 2.0 server
│   └── node/        — Miner, state machine, CLI entrypoint
```

### Crate Dependencies

```
core ← crypto ← consensus ← execution ← node → rpc
                                     ← storage ← node
                                     ← wallet ← node
                                     ← networking ← node
```

---

## Cryptographic Stack

| Layer | Algorithm | Purpose |
|---|---|---|
| **Hashing** | Blake3-256 (32 bytes) | Block hashes, transaction IDs, ObjectIds, state roots |
| **Signing** | ed25519 (via ed25519-dalek) | Transaction authentication and validator block signing |
| **Key Derivation** | SLIP-0010 + HMAC-SHA512 | BIP-44 path m/44'/999'/0'/0' |
| **Mnemonic** | BIP-39 (24-word, 256-bit entropy) | Wallet recovery |

### ObjectId

Account addresses are **Blake3-256 hashes of ed25519 public keys** — not the public keys themselves. This provides a 32-byte uniform address space and an extra hash layer.

### Key Derivation

A single ed25519 keypair — derived deterministically from the BIP-39 mnemonic via SLIP-0010 — handles both transaction signing and validator block signing. Full wallet recovery is possible from the mnemonic alone; no separate backup file is needed.

---

## Consensus

### Proof of Work

- **Algorithm**: Autolykos-inspired memory-hard PoW (ASIC-resistant)
- **Block target time**: 120 seconds
- **Difficulty retarget**: Every 1,024 blocks, with [current/4, current×4] clamping
- **Discovery bonus**: Sub-linear reward scaling that rewards miners who discover blocks faster than expected, without inflationary spirals

### Natural Equilibrium (No Hard Cap)

There is no fixed supply cap. Instead, OPL models real gold dynamics:

- **Issuance**: `BASE_REWARD / difficulty` — as difficulty rises, per-block issuance falls
- **Burning**: All transaction fees are permanently destroyed, reducing circulation
- **Deflationary pressure**: As more OPL is issued and more fees are burned, the circulating supply can actually decrease over time — just like gold jewelry being melted or coins being lost

The BASE_REWARD of 440 OPL/block at minimum difficulty is derived from:

```
annual_gold_production ≈ 3,630 tonnes
total_above_ground     ≈ 219,891 tonnes
annual_granular_change ≈ floor(3,630 × 32,150.7 / 212,000) = ... ultimately 440 OPL
```

This ratio mirrors how new gold production constantly dilutes the above-ground stock.

### Proof of Stake Transition

OPL transitions smoothly from PoW to PoS as validators bond stake:

- **Stake coverage** = `total_bonded / total_issued`
- When stake coverage > 0, validators begin producing blocks alongside miners
- The PoW/PoS reward split tracks stake coverage continuously — no thresholds, no governance votes
- Validators earn proportional to **weight** = `Σ entry.stake × (1 + ln(1 + entry.age_years))` — each bond entry has its own seniority clock

---

## Block Structure

```rust
Block {
    header: BlockHeader {
        height: u64,                  // 0 for genesis, 1, 2, ...
        previous_hash: Hash,          // Blake3-256 of prior block header
        state_root: Hash,             // Blake3-256 of state after this block
        transaction_root: Hash,       // Blake3-256 commitment of all tx IDs
        timestamp: u64,               // UNIX epoch seconds
        difficulty: u64,               // Effective difficulty for this block
        pow_proof: Option<Vec<u8>>,   // Autolykos nonce (None for genesis/PoS)
        validator_signature: Option<Vec<u8>>, // ed25519 signature
    },
    transactions: Vec<Transaction>,
}
```

### Block Hash

Block hashes are computed as `Blake3-256(header_bytes)` where `header_bytes` is the Borsh-serialized `BlockHeader` with `pow_proof` and `validator_signature` set to `None`. This means:

- The hash is determined before mining begins
- The PoW proof and validator signature are attached after the hash is computed
- Genesis block hash is computed from the ceremony configuration, not hardcoded

---

## Transactions

### Types

| Action | Description |
|---|---|
| `Transfer { recipient, amount }` | Move OPL from sender to recipient |
| `ValidatorBond { amount }` | Lock OPL as validator stake, or top-up existing validator (min 100 OPL per entry) |
| `ValidatorUnbond { bond_id }` | Unbond a specific entry by `bond_id`, returning that entry's stake to sender |

### Transaction Lifecycle

1. **Create**: Wallet signs transaction with ed25519
2. **Submit**: Transaction enters the mempool via RPC
3. **Order**: Mempool sorts by fee priority (market-driven, no minimum)
4. **Include**: Miner/validator selects transactions into a block
5. **Execute**: Dispatcher applies state transitions atomically:
   - **Transfer**: Debit sender, credit recipient, burn fee
   - **Bond**: Debit sender, create new bond entry (or top-up existing validator), burn fee
   - **Unbond**: Return specific entry's stake to sender balance, remove entry, burn fee
6. **Persist**: Block and state written to RocksDB atomically

### Fee Model

All fees are **permanently burned** — not transferred to validators or miners. This creates continuous deflationary pressure:

- **No minimum fee**: The mempool accepts any transaction, ordered by fee priority
- **No fee schedule**: Market determines what fee level gets included
- **Validator income**: Comes entirely from block rewards, not fees

---

## RPC API

The node exposes a JSON-RPC 2.0 server on port 4171.

### Read

| Method | Parameters | Description |
|---|---|---|
| `opl_getBlockHeight` | _(none)_ | Current chain height |
| `opl_getChainInfo` | _(none)_ | Chain statistics (height, difficulty, supply, validators) |
| `opl_getNetworkVersion` | _(none)_ | Protocol version string |
| `opl_getBalance` | `["object_id_hex"]` | Account balance (flakes and OPL) |
| `opl_getAccount` | `["object_id_hex"]` | Full account details (balance, nonce) |
| `opl_getBlockByHeight` | `[height]` | Full block at given height |
| `opl_getBlockByHash` | `["hex_hash"]` | Full block by Blake3 hash |
| `opl_getLatestBlocks` | `[count]` or `null` | Recent blocks (default 10) |
| `opl_getTransaction` | `["tx_id_hex"]` | Transaction by ID with status |
| `opl_getMempoolStatus` | _(none)_ | Pending transaction count and size |
| `opl_getSupply` | _(none)_ | Issued, burned, and circulating breakdown |
| `opl_getDifficulty` | _(none)_ | Current difficulty and retarget info |
| `opl_getValidators` | _(none)_ | Active validator set with per-entry bond details |

### Write

| Method | Parameters | Description |
|---|---|---|
| `opl_sendTransaction` | `["borsh_hex_string"]` | Submit a Borsh-hex-encoded signed transaction |

### Mining

| Method | Parameters | Description |
|---|---|---|
| `opl_getMiningJob` | _(none)_ | Block template for external miners |
| `opl_submitSolution` | `["borsh_hex_string"]` | Submit mined block _(placeholder — not yet fully wired)_ |

### Examples

```bash
# Get chain info
curl -X POST http://localhost:4171/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"opl_getChainInfo","params":null,"id":1}'

# Check balance
curl -X POST http://localhost:4171/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"opl_getBalance","params":["<hex_object_id>"],"id":2}'

# Get block by height
curl -X POST http://localhost:4171/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"opl_getBlockByHeight","params":[42],"id":3}'

# Get latest 5 blocks
curl -X POST http://localhost:4171/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"opl_getLatestBlocks","params":[5],"id":4}'

# Get a transaction
curl -X POST http://localhost:4171/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"opl_getTransaction","params":["<tx_id_hex>"],"id":5}'

# Get mempool status
curl -X POST http://localhost:4171/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"opl_getMempoolStatus","params":null,"id":6}'

# Get supply breakdown
curl -X POST http://localhost:4171/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"opl_getSupply","params":null,"id":7}'

# Get mining job
curl -X POST http://localhost:4171/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"opl_getMiningJob","params":null,"id":8}'

# Health check
curl http://localhost:4171/health
```

---

## Wallet Key Derivation

### BIP-44 Path

```
m / 44' / 999' / account' / 0'
│    │     │       │        └── change (always 0' for ed25519)
│    │     │       └── account index (0, 1, 2, ...)
│    │     └── SLIP-0044 coin type for Opolys
│    └── BIP-44 purpose (always 44' for BIP-44)
└── master key (hardened)
```

### Recovery Rules

| Key Type | Recoverable from Mnemonic? | Backup Required |
|---|---|---|
| ed25519 | **Yes** — deterministic SLIP-0010 derivation | Mnemonic phrase only |

### Mnemonic Format

- 24-word BIP-39 phrase (256 bits of entropy)
- Standard English wordlist with checksum validation
- Optional passphrase (BIP-39 password) for additional security

---

## Genesis Ceremony

The genesis block is created from a `GenesisAttestation` containing:

- ** ceremony_timestamp**: UNIX timestamp of the ceremony
- **gold_spot_price_usd_cents**: LBMA gold price at ceremony time
- **annual_production_tonnes**: USGS annual mine production (~3,630 t)
- **total_above_ground_tonnes**: WGC above-ground stock (~219,891 t)
- **Response hashes**: Blake3-256 hashes of the raw LBMA, USGS, and WGC responses
- **Derivation formula**: The mathematical formula linking gold data to BASE_REWARD

The genesis block has:
- Height 0
- Zero previous hash
- No transactions
- No PoW proof
- State root computed from ceremony parameters and protocol constants

---

## Storage

Opolys uses **RocksDB** with Borsh serialization for all persistent data:

| Column Family | Key | Value |
|---|---|---|
| `blocks` | `block_<height>` | Borsh-serialized `Block` |
| `accounts` | `account_<hex_object_id>` | Borsh-serialized `Account` |
| `validators` | `validator_<hex_object_id>` | Borsh-serialized `ValidatorInfo` |
| `chain_state` | `chain_state` | Borsh-serialized `PersistedChainState` |

State is saved atomically after each block is applied.

---

## Difficulty & Emission Formulas

### Block Reward

```
block_reward = BASE_REWARD / effective_difficulty × discovery_bonus
```

Where:
- `BASE_REWARD = 440 × FLAKES_PER_OPL = 440,000,000 flakes`
- `effective_difficulty = max(retarget, consensus_floor, MIN_DIFFICULTY)`
- `discovery_bonus ≈ √(MAX / (difficulty × hash))` — sub-linear reward for lucky miners

### Consensus Floor

```
consensus_floor = total_issued / bonded_stake
```

This creates a natural equilibrium: as more OPL is issued relative to bonded stake, difficulty must rise, slowing emission.

### Difficulty Retarget

Every `RETARGET_EPOCH` (1,024) blocks:

```
new_difficulty = old_difficulty × (actual_time / expected_time)
new_difficulty = clamp(new_difficulty, old/4, old×4)
new_difficulty = max(new_difficulty, MIN_DIFFICULTY)
```

### Validator Weight

```
weight = Σ entry.stake × (1 + ln(1 + entry.age_years))
```

Each validator can hold multiple bond entries, each with its own `bond_id`, stake, and seniority clock. Seniority starts at zero for each new entry (top-up bonds earn no bonus initially). Logarithmic — the marginal gain diminishes over time, so early validators earn more per-coin but never dominate permanently.

To unbond, specify which entry by `bond_id`. Invalid bond IDs fail with no fee burn. Pools are a market innovation — the protocol provides per-entry bonds, community builds pooling off-chain.

---

## Constants Reference

| Constant | Value | Description |
|---|---|---|
| `FLAKES_PER_OPL` | 1,000,000 | Smallest unit ratio |
| `BASE_REWARD` | 440,000,000 flakes (440 OPL) | Gold-derived block reward |
| `MIN_DIFFICULTY` | 1 | Floor difficulty |
| `RETARGET_EPOCH` | 1,024 blocks | Difficulty adjustment interval |
| `BLOCK_TARGET_TIME_SECS` | 120 | Target block time |
| `MIN_BOND_STAKE` | 100,000,000 flakes (100 OPL) | Minimum validator bond |
| `NETWORK_PROTOCOL_VERSION` | `"opolys/0.1.0"` | Protocol identifier |
| `PENNYWEIGHTS_PER_OPL` | 100 | 1 dwt = 0.01 OPL |
| `GRAINS_PER_OPL` | 10,000 | 1 gr = 0.0001 OPL |
| `DEFAULT_LISTEN_PORT` | 4170 | P2P listen port |
| `BLOCK_CAPACITY_RATE` | 1.5 | Max transactions per second |
| `POS_FINALITY_BLOCKS` | 720 | PoS finality depth |

---

## Development

### Project Structure

Each crate has a clear responsibility:

| Crate | Purpose | Key Types |
|---|---|---|
| `opolys-core` | Shared types, constants, errors | `Hash`, `ObjectId`, `Transaction`, `Block`, `FlakeAmount` |
| `opolys-crypto` | Cryptographic primitives | `Blake3Hasher`, `verify_ed25519` |
| `opolys-consensus` | Consensus engine | `AccountStore`, `ValidatorSet`, `Mempool`, difficulty, emission, genesis |
| `opolys-execution` | Transaction dispatch | `TransactionDispatcher`, `ApplyResult` |
| `opolys-storage` | RocksDB persistence | `BlockchainStore`, `PersistedChainState` |
| `opolys-networking` | P2P networking | `GossipConfig`, `SyncConfig`, `DiscoveryConfig` |
| `opolys-wallet` | Key management | `KeyPair`, `Bip39Mnemonic`, `TransactionSigner` |
| `opolys-rpc` | JSON-RPC server | `RpcState`, `ChainInfo`, `handle_jsonrpc` |
| `opolys-node` | Node orchestration | `OpolysNode`, `ChainState`, `NodeConfig`, mining loop |

### Building

```bash
# Debug build (fast compilation)
cargo build

# Release build (optimized)
cargo build --release

# Run with logging
RUST_LOG=debug cargo run
```

### Testing

```bash
# All tests
cargo test

# Individual crate
cargo test -p opolys-consensus

# Specific test
cargo test -p opolys-core -- test_constants_are_consistent

# With output
cargo test -- --nocapture
```

### Linting

```bash
cargo clippy --all-targets --all-features
cargo fmt --check
```

---

## Roadmap

| Phase | Status | Description |
|---|---|---|
| **1. Storage & Persistence** | ✅ | RocksDB, Borsh serialization, atomic state saves |
| **1b. Bug Fixes + Wallet** | ✅ | ValidatorBond amount, block hash chain, BIP-39/SLIP-0010 |
| **3. RPC + TX Lifecycle** | ✅ | Axum JSON-RPC server, transaction submission, mempool |
| **7a. Security Hardening** | 🔜 | Code audit, fuzz testing, overflow checks (before genesis) |
| **6. Genesis Ceremony** | 🔜 | Lock in real data on hardened code |
| **2. Networking** | 📋 | libp2p gossip, sync, discovery |
| **4. Staking & PoS** | 📋 | Validator bonding, block production, transitions |
| **5. Wallet CLI** | 📋 | Command-line wallet with mnemonic management |
| **7b. Security Audit** | 📋 | User-facing attack surface, CLI hardening (2nd pass) |
| **8. Testnet** | 📋 | Public testnet deployment |
| **9. Mainnet** | 📋 | Genesis ceremony, mainnet launch |

---

## License

MIT