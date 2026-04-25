# THE PLAN — Opolys ($OPL)

**Complete technical roadmap, calculations, constants, and build sequence.**
This document is the single source of truth for the entire project.

---

## Table of Contents

1. [Vision & Philosophy](#1-vision--philosophy)
2. [Currency Model](#2-currency-model)
3. [All Constants](#3-all-constants)
4. [Cryptographic Stack](#4-cryptographic-stack)
5. [Consensus Model](#5-consensus-model)
6. [Gold-Derived Emission](#6-gold-derived-emission)
7. [Difficulty & Retargeting](#7-difficulty--retargeting)
8. [Validator Staking (PoS)](#8-validator-staking-pos)
9. [FIFO Unbonding](#9-fifo-unbonding)
10. [Fees & Burning](#10-fees--burning)
11. [EVO-OMAP Proof-of-Work](#11-evo-omap-proof-of-work)
12. [Vein Yield](#12-vein-yield)
13. [Block Structure](#13-block-structure)
14. [Transaction Model](#14-transaction-model)
15. [RPC API](#15-rpc-api)
16. [Wallet Key Derivation](#16-wallet-key-derivation)
17. [Networking (libp2p)](#17-networking-libp2p)
18. [Storage](#18-storage)
19. [Architecture & Crate Map](#19-architecture--crate-map)
20. [Build Sequence](#20-build-sequence)
21. [Test Count](#21-test-count)

---

## 1. Vision & Philosophy

Opolys is **decentralized digital gold** — a pure coin with:

- **No tokens, no assets, no governance, no hardcoded caps**
- Every parameter emerges from mathematics or market forces
- Supply grows via block rewards (mirroring real gold production) and contracts via fee burning (mirroring gold attrition)
- Only double-signing gets slashed — no reversal windows, no confiscation
- Mining is opt-in via `--mine` flag, not default
- Community builds explorers, wallets, mining pools — the core is the protocol layer (like Bitcoin Core)

**Tech direction**: EVO-OMAP PoW, BLS signatures, VRF, stealth addresses, viewing keys, Poseidon hash. NO WASM, NO object model, NO multi-asset, NO governance. The coin stays "just a coin" but with better privacy and decentralization primitives.

---

## 2. Currency Model

OPL uses 6 decimal places, named after real gold weight units:

| Unit | Flakes | OPL | Example |
|---|---|---|---|
| **OPL** | 1,000,000 | 1 | `1.000000 OPL` |
| **Flake** | 1 | 0.000001 | `0.000001 OPL` |

There is only one sub-unit: the Flake. No Pennyweight or Grain. All on-chain arithmetic uses `FlakeAmount` (u64).

---

## 3. All Constants

From `crates/core/src/constants.rs`:

| Constant | Value | Description |
|---|---|---|
| `CURRENCY_NAME` | `"Opolys"` | Human-readable name |
| `CURRENCY_TICKER` | `"OPL"` | Exchange ticker |
| `CURRENCY_SMALLEST_UNIT` | `"Flake"` | Name of 1/1,000,000 OPL |
| `FLAKES_PER_OPL` | `1_000_000` | Fundamental unit ratio |
| `DECIMAL_PLACES` | `6` | Always 6 decimal places |
| `BASE_REWARD` | `312,000,000` Flakes (312 OPL) | Gold-derived block reward base |
| `MIN_DIFFICULTY` | `1` | Mathematical floor (not a cap) |
| `EPOCH` | `1,024` blocks | Unified epoch for retarget, dataset regen, unbonding |
| `UNBONDING_DELAY_BLOCKS` | `1,024` | One epoch delay for unbonding |
| `MIN_FEE` | `1` Flake | Floor for market-driven fees |
| `MIN_BOND_STAKE` | `1,000,000` Flakes (1 OPL) | Minimum per new bond entry |
| `BLOCK_VERSION` | `1` | Current block header version |
| `SIGNATURE_TYPE_ED25519` | `0` | ed25519 signature type constant |
| `EXTENSION_TYPE_NONE` | `0` | No extension data |
| `EXTENSION_TYPE_ROLLUP` | `1` | Rollup data (reserved) |
| `POS_FINALITY_BLOCKS` | `3` | PoS finality depth |
| `BLOCK_TARGET_TIME_SECS` | `84` | One block every ~84 seconds |
| `NETWORK_PROTOCOL_VERSION` | `"1.0.0"` | Protocol version string |
| `DEFAULT_LISTEN_PORT` | `4170` | P2P listen port |

---

## 4. Cryptographic Stack

### Current (Implemented)

| Layer | Algorithm | Purpose |
|---|---|---|
| **Hashing** | Blake3-256 (32 bytes) | Block hashes, transaction IDs, ObjectIds, state roots, Merkle roots |
| **PoW Finalization** | SHA3-256 | EVO-OMAP final hash |
| **PoW Inner Loop** | Blake3 (XOF mode) | EVO-OMAP dataset generation, branch mixing |
| **Signing** | ed25519 (via ed25519-dalek) | Transaction auth and validator block signing |
| **Key Derivation** | SLIP-0010 + HMAC-SHA512 | BIP-44 path: `m/44'/999'/0'/0'` |
| **Mnemonic** | BIP-39 (24-word, 256-bit entropy) | Wallet recovery |

### Planned (Not Yet Implemented)

| Layer | Algorithm | Purpose |
|---|---|---|
| **Validator Signatures** | BLS12-381 | Signature aggregation for efficient PoS attestation |
| **Block Producer Selection** | VRF | Unpredictable, verifiable validator selection |
| **Privacy (L1)** | Stealth addresses | Receiver privacy via one-time addresses |
| **Privacy (L2)** | Viewing keys | Selective transaction visibility |
| **ZK Foundation** | Poseidon hash | ZK-friendly hash for future SNARKs/STARKs |

### ObjectId

Account addresses are **Blake3-256 hashes of ed25519 public keys** — not the public keys themselves. This provides a 32-byte uniform address space and an extra hash layer.

---

## 5. Consensus Model

Opolys uses **hybrid PoW/PoS** with a smooth transition:

- **PoW blocks** are mined when difficulty is high relative to stake coverage
- **PoS blocks** are produced by validators as stake coverage grows
- The split is **continuous** — no thresholds, no governance votes

### Consensus Phase Selection

- `stake_coverage = total_bonded / total_issued`, clamped to [0.0, 1.0]
- `pow_share = 1.0 - stake_coverage` (miner reward fraction)
- `pos_share = stake_coverage` (validator reward fraction)

---

## 6. Gold-Derived Emission

### Derivation

| Metric | Value | Source |
|---|---|---|
| Annual gold production | 3,630 tonnes | USGS/WGC 2024-2025 avg |
| Annual production in troy oz | ~116,707,041 | 3,630 × 32,150.7 |
| Blocks per year | 374,256 | 365.25 × 1,024 |
| **BASE_REWARD** | **312 OPL** | floor(116,707,041 / 374,256) |

### Block Reward Formula

```
block_reward = (BASE_REWARD / difficulty) × vein_yield
```

Where:
- `BASE_REWARD = 312 × FLAKES_PER_OPL = 312,000,000 flakes`
- `difficulty` = effective difficulty (max of retarget, consensus_floor, MIN_DIFFICULTY)
- `vein_yield` = `1 + ln(target / hash_int)` (see Section 12)

### Natural Equilibrium

There is **no hard cap**. Issuance shrinks as difficulty rises (like gold getting harder to mine). Fee burning reduces supply. The two forces reach a natural equilibrium.

---

## 7. Difficulty & Retargeting

### Retarget Algorithm

Every `EPOCH` (1,024) blocks:

```
new_difficulty = old_difficulty × (expected_time / actual_time)
```

If blocks were too fast (actual < expected), difficulty increases. If too slow (actual > expected), difficulty decreases.

**No maximum clamp.** The only floor is `MIN_DIFFICULTY` (1), which is a mathematical requirement since difficulty 0 would make all hashes valid.

### Consensus Floor

```
consensus_floor = total_issued / bonded_stake
```

When `bonded_stake = 0`, floor = 0 (no validators yet).

### Effective Difficulty

```
effective_difficulty = max(retarget, consensus_floor, MIN_DIFFICULTY)
```

---

## 8. Validator Staking (PoS)

### Bond Lifecycle

1. `ValidatorBond { amount }` — Lock `amount` OPL as stake. Creates a new entry if the validator already exists (top-up).
2. `ValidatorUnbond { amount }` — Withdraw `amount` OPL using **FIFO order** (see Section 9).
3. **Slashing** — Only for double-signing. All entries' stakes are **burned** (not confiscated). Validator status set to `Slashed`.

### Per-Entry Weight

Each `BondEntry` has its own seniority clock:

```
entry_weight = entry.stake × (1 + ln(1 + entry.age_years))
```

Logarithmic seniority means older entries earn more per-coin, but the marginal gain diminishes — preventing permanent dominance by early stakers.

### Block Producer Selection

Weighted random sampling from active validators. Seed is derived from on-chain entropy.

### Minimum Bond

New bond entries require at least `MIN_BOND_STAKE` (1 OPL). Residuals from FIFO splitting are exempt from this minimum.

---

## 9. FIFO Unbonding

### `ValidatorUnbond { amount: FlakeAmount }`

Withdraws `amount` OPL using **FIFO order** — oldest entries consumed first:

1. Sort entries by `bonded_at_timestamp` (oldest first)
2. Consume entries from the front:
   - If `entry.stake <= remaining_amount`: consume entire entry
   - If `entry.stake > remaining_amount`: **split** the entry
     - Return `remaining_amount` to sender
     - Keep residual with **original timestamp** (preserves seniority)
3. Residuals keep their original `bonded_at_timestamp`
4. Auto-merge entries with the same `bonded_at_timestamp`

Unbonding stake still earns rewards during the 1,024 block delay.

---

## 10. Fees & Burning

All transaction fees are **permanently burned** — not collected by validators or miners.

- **Suggested fee**: `suggested_fee` field in `BlockHeader`, computed via EMA of previous block's fees. Starts at `MIN_FEE` (1 Flake).
- **No minimum fee beyond 1 Flake**: Market determines inclusion
- **Validator income**: Block rewards only, not fees
- **Deflationary**: Fee burning reduces circulating supply

Invalid transactions (wrong nonce, insufficient balance, invalid unbond amount): no fee burn, no nonce advance.

---

## 11. EVO-OMAP Proof-of-Work

EVO-OMAP (EVOlutionary Oriented Memory-hard Algorithm for Proof-of-work) is the mining algorithm.

### Key Properties

| Property | Value | Implication |
|---|---|---|
| Memory Footprint | 256 MiB | ASICs require expensive on-chip SRAM |
| Memory Access | Read-write per step | Cannot be computed with DRAM alone |
| Branch Factor | 4-way | GPU warp efficiency reduced |
| Execution Model | Superscalar (8 instructions/step) | Data-dependent operands |
| State Size | 512 bits | Full in-register execution |
| Dataset Chaining | Sequential | Nodes form a chain, no parallel precomputation |
| Inner Hash | Blake3 (XOF) | Fast inner loop |
| Final Hash | SHA3-256 | Different security margin from inner |

### Parameters

| Parameter | Value | Description |
|---|---|---|
| `NODE_SIZE` | 1,048,576 bytes (1 MiB) | Bytes per dataset node |
| `NUM_NODES` | 256 | Total nodes = 256 MiB dataset |
| `NUM_STEPS` | 4,096 | Execution steps per hash |
| `PROGRAM_LENGTH` | 8 | Instructions per program |
| `BRANCH_WAYS` | 4 | Branch variants |
| `EPOCH_LENGTH` | 1,024 | Blocks per epoch (matches EPOCH constant) |

### Mining API

```rust
// Single-threaded mining
let (nonce, attempts) = mine(header, height, difficulty, max_attempts);

// Multi-threaded mining (uses rayon)
let (nonce, attempts) = mine_parallel(header, height, difficulty, max_attempts, num_threads);

// Full verification (requires 256 MiB dataset)
let valid = verify(header, height, nonce, difficulty);

// Light verification (on-demand node reconstruction, saves memory)
let valid = verify_light(header, height, nonce, difficulty);

// Dataset caching (avoids regeneration within epoch)
let mut cache = DatasetCache::new();
let dataset = cache.get_dataset(height);
```

---

## 12. Vein Yield

Vein yield replaces discovery bonus with a mathematically cleaner formula:

```
vein_yield = 1 + ln(target / hash_int)
```

Where:
- `target = u64::MAX / difficulty`
- `hash_int` = first 8 bytes of the EVO-OMAP PoW hash, interpreted as big-endian u64

This gives a natural distribution: most blocks earn ~2x BASE_REWARD, exceptionally lucky blocks earn more. The expected value is approximately 2.0.

Implementation uses `f64::ln()` with deterministic rounding. IEEE 754 guarantees identical results across all platforms.

---

## 13. Block Structure

```rust
BlockHeader {
    version: u32,                       // Protocol version (currently 1)
    height: u64,                        // 0 for genesis
    previous_hash: Hash,                // Blake3-256 of prior block
    state_root: Hash,                   // Blake3-256 of post-execution state
    transaction_root: Hash,             // Blake3-256 commitment of tx IDs
    timestamp: u64,                      // UNIX epoch seconds
    difficulty: u64,                     // Effective difficulty
    suggested_fee: FlakeAmount,          // EMA of previous block's fees
    extension_root: Option<Hash>,       // Reserved for rollups
    pow_proof: Option<Vec<u8>>,          // EVO-OMAP nonce (None for PoS)
    validator_signature: Option<Vec<u8>>, // ed25519 signature
}

Block {
    header: BlockHeader,
    transactions: Vec<Transaction>,
}
```

### Block Hash

`Blake3-256(header_bytes)` where `pow_proof` and `validator_signature` are set to `None` before hashing. The `version`, `suggested_fee`, and `extension_root` fields are included in the hash.

---

## 14. Transaction Model

### Types

```rust
enum TransactionAction {
    Transfer { recipient: ObjectId, amount: FlakeAmount },
    ValidatorBond { amount: FlakeAmount },
    ValidatorUnbond { amount: FlakeAmount },  // FIFO unbonding
}
```

### Transaction Structure

```rust
Transaction {
    tx_id: ObjectId,           // Blake3-256(sender || action || fee || nonce)
    sender: ObjectId,          // Blake3-256(ed25519_pubkey)
    action: TransactionAction,
    fee: FlakeAmount,           // Burned, not collected
    signature: Vec<u8>,        // ed25519 signature
    signature_type: u8,         // 0 = ed25519 (reserved for post-quantum)
    nonce: u64,                 // Replay protection
    data: Vec<u8>,             // Arbitrary attachment
}
```

---

## 15. RPC API

JSON-RPC 2.0 server on port 4171 (default: listen_port + 1).

### Mining Endpoints

| Method | Parameters | Description |
|---|---|---|
| `opl_getMiningJob` | _(none)_ | Block template for external miners |
| `opl_submitSolution` | `["borsh_hex_string"]` | Submit mined block |

### Read Endpoints

| Method | Parameters | Description |
|---|---|---|
| `opl_getBlockHeight` | _(none)_ | Current chain height |
| `opl_getChainInfo` | _(none)_ | Chain stats including `suggested_fee` |
| `opl_getBalance` | `["object_id_hex"]` | Account balance |
| `opl_getValidators` | _(none)_ | Validator set with FIFO bond entries |

### Write Endpoints

| Method | Parameters | Description |
|---|---|---|
| `opl_sendTransaction` | `["borsh_hex_string"]` | Submit signed transaction |

---

## 16. Wallet Key Derivation

### BIP-44 Path

```
m / 44' / 999' / account' / 0'
```

SLIP-0044 coin type 999 for OPL. Single ed25519 key handles both transaction signing and validator block signing. Full wallet recovery from mnemonic alone.

---

## 17. Networking (libp2p)

- **Transport**: libp2p 0.54 with QUIC, TCP, noise, yamux, relay client
- **Discovery**: Kademlia DHT (bucket size 20) + identify protocol
- **Gossip**: Gossipsub for block/transaction propagation (`opolys/tx/v1`, `opolys/block/v1`)
- **Sync**: CBOR request-response protocol for block download (`/opolys/sync/1`)
- **Ping**: Liveness checks with 30s interval, 10s timeout

All five protocols composed into `OpolysBehaviour` via `#[derive(NetworkBehaviour)]`. Events routed through `OpolysBehaviourEvent` → `OpolysNetworkEvent` to the node's main event loop.

**Wired features:**
- Incoming gossipsub **transactions** → deserialized and added to mempool
- Incoming gossipsub **blocks** → deserialized, validated, and applied (with height dedup)
- Incoming **sync requests** → serve blocks from RocksDB storage, respond via `ResponseChannel`
- Incoming **sync responses** → apply received blocks to catch up to chain tip
- Mined/submitted blocks → broadcast to P2P peers via gossipsub
- Identify protocol → adds peer addresses to Kademlia DHT

---

## 18. Storage

Opolys uses **RocksDB** with **Borsh** serialization. State is saved atomically after each block. Tests use `tempfile::tempdir()` for isolation.

---

## 19. Architecture & Crate Map

```
Opolys/
├── Cargo.toml                         # Workspace
├── crates/
│   ├── core/src/
│   │   ├── constants.rs                # EPOCH, MIN_FEE, MIN_BOND_STAKE, BLOCK_VERSION, etc.
│   │   ├── types.rs                    # Hash, ObjectId, BlockHeader (with version/suggested_fee/extension_root)
│   │   └── errors.rs                   # OpolysError enum
│   ├── crypto/src/
│   │   ├── hash.rs                     # Blake3-256, SHA3-256, Blake3 XOF
│   │   ├── signing.rs                  # ed25519 verification
│   │   └── key.rs                      # KeyPair
│   ├── consensus/src/
│   │   ├── account.rs                  # AccountStore with fee-burning transfers
│   │   ├── block.rs                    # compute_block_hash(), compute_transaction_root()
│   │   ├── difficulty.rs               # Retarget (expected/actual), consensus floor, no clamp
│   │   ├── emission.rs                 # Vein yield, ln_milli (f64), suggested_fee EMA
│   │   ├── mempool.rs                  # Fee-priority mempool
│   │   ├── pos.rs                      # ValidatorSet with FIFO unbonding
│   │   ├── pow.rs                      # EVO-OMAP PowContext, verify, compute_pow_hash_value
│   │   └── genesis.rs                  # Genesis block construction
│   ├── storage/src/store.rs            # RocksDB persistence with suggested_fee
│   ├── execution/src/dispatcher.rs      # TransactionDispatcher (Transfer, Bond, Unbond FIFO)
│   ├── wallet/src/
│   │   ├── bip39.rs                    # BIP-39 + SLIP-0010 derivation
│   │   ├── signing.rs                  # TransactionSigner with signature_type
│   │   ├── key.rs                      # KeyPair
│   │   └── account.rs                  # AccountInfo
│   ├── rpc/src/server.rs              # JSON-RPC 2.0 with suggested_fee, MiningJob
│   ├── networking/src/
│   │   ├── behaviour.rs              # OpolysBehaviour (gossipsub+kad+identify+ping+request_response)
│   │   ├── network.rs               # OpolysNetwork, SwarmTask, NetworkCommand, event routing
│   │   ├── gossip.rs                # GossipConfig (tx/block topics, max message size)
│   │   ├── sync.rs                  # SyncRequest, SyncResponse (CBOR), SyncConfig
│   │   └── discovery.rs            # DiscoveryConfig (Kad bucket size)
│   └── node/src/
│       ├── main.rs                   # CLI, P2P event loop, mempool wiring, block sync
│       └── node.rs                   # OpolysNode with PowContext, vein yield, apply_block
```

### Key External Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `borsh` | 1.5 | Serialization (consensus-critical) |
| `blake3` | 1.8 | Hashing |
| `sha3` | 0.10 | EVO-OMAP finalization |
| `ed25519-dalek` | 2.1 | Transaction signing |
| `rocksdb` | 0.22 | Persistent storage |
| `libp2p` | 0.54 | P2P networking |
| `tokio` | 1 | Async runtime |
| `axum` | 0.8 | HTTP/RPC server |
| `evo-omap` | local | Proof-of-work algorithm |
| `rayon` | 1.12 | Parallel mining |

---

## 20. Build Sequence

| Phase | Status | Description |
|---|---|---|
| 1: Core types | **DONE** | Hash, ObjectId, Transaction, Block, constants |
| 2: Crypto | **DONE** | Blake3, ed25519, key derivation |
| 3: Consensus | **DONE** | EVO-OMAP PoW, vein yield, difficulty, FIFO unbonding |
| 4: Storage | **DONE** | RocksDB with all column families |
| 5: Execution | **DONE** | Transaction dispatcher (Transfer, Bond, Unbond) |
| 6: Wallet | **DONE** | BIP-39, SLIP-0010, ed25519 signing |
| 7: RPC | **DONE** | JSON-RPC 2.0 with mining endpoints |
| 8: Node | **DONE** | Full node with mining loop and block application |
| 9: Networking | **DONE** | P2P gossip/sync/discovery wired to node |
| 10: Staking | **IN PROGRESS** | `--validate` flag, public_key in Account, PoS signing |
| 11: Security | **IN PROGRESS** | Block validation, tx_id verification, chain sync |
| 12: Testnet | **PLANNED** | Deploy and test |
| 13: Mainnet | **PLANNED** | Genesis ceremony and launch |

---

## 21. Test Count

**144 tests passing** across all crates (1 mining integration test `#[ignore]`d for requiring real PoW).

---

## 22. Block & Transaction Validation

### Block Validation (`validate_block`)

Every block applied to the chain must pass these checks:

1. **Version**: Must match `BLOCK_VERSION` (currently 1)
2. **Height**: Must equal `parent_height + 1`
3. **Previous hash**: Must match parent's hash (or `Hash::zero()` for genesis)
4. **Timestamp**: Must be strictly greater than parent timestamp, and within `MAX_FUTURE_BLOCK_TIME_SECS` (5 min) of wall clock
5. **Difficulty**: Must match the expected next difficulty from retargeting
6. **Transaction count**: Must not exceed `MAX_TRANSACTIONS_PER_BLOCK` (10,000)
7. **Block size**: Must not exceed `MAX_BLOCK_SIZE_BYTES` (10 MiB)
8. **Transaction root**: Must match `compute_transaction_root(block.transactions)`
9. **No duplicate transactions**: Each `tx_id` must be unique within the block
10. **Transaction data size**: Each `tx.data` must not exceed `MAX_TX_DATA_SIZE_BYTES` (1 KiB)
11. **Fee minimum**: Each transaction fee must be at least `MIN_FEE` (1 Flake)
12. **PoW proof**: For PoW blocks, EVO-OMAP proof must satisfy the difficulty target

### Transaction Verification (`verify_transaction`)

1. **tx_id integrity**: Recomputed from (sender, action, fee, nonce) must match declared `tx_id`
2. **signature_type**: Must be `SIGNATURE_TYPE_ED25519` (0)
3. **ed25519 signature**: Planned — requires public key storage in Account (in progress)

### Chain Sync

- On peer connection, node requests blocks from `current_height + 1` onwards
- Sync responses are deserialized and applied sequentially
- Block sync requests served from RocksDB storage via `ResponseChannel`

---

## 23. New Constants

| Constant | Value | Description |
|---|---|---|
| `MAX_TRANSACTIONS_PER_BLOCK` | 10,000 | Max transactions per block |
| `MAX_BLOCK_SIZE_BYTES` | 10,485,760 | 10 MiB max block size |
| `MAX_TX_DATA_SIZE_BYTES` | 1,024 | 1 KiB max transaction data field |
| `MAX_FUTURE_BLOCK_TIME_SECS` | 300 | 5 min max future block time skew |

---

## Key Formulas Reference

### Block Reward
```
vein_yield = 1 + ln(target / hash_int)        // f64, rounded to nearest milli
block_reward = (BASE_REWARD / difficulty) × vein_yield
```

### Effective Difficulty
```
effective_difficulty = max(retarget, consensus_floor, MIN_DIFFICULTY)
```

### Difficulty Retarget
```
new_difficulty = old_difficulty × expected_time / actual_time
```
No maximum clamp. Floor is MIN_DIFFICULTY (1).

### Consensus Floor
```
consensus_floor = total_issued / bonded_stake
```

### Suggested Fee
```
suggested_fee = EMA(previous_fees, previous_suggested_fee)
             = (current + 9 × old) / 10, floored at MIN_FEE
```

### Validator Weight
```
entry_weight = entry.stake × (1 + ln(1 + entry.age_years))
```

### Stake Coverage
```
stake_coverage = min(1.0, total_bonded / total_issued)
```

### EVO-OMAP PoW Verification
```
target = u64::MAX / difficulty
valid if: u64(pow_hash[..8]) < target
pow_hash = SHA3-256(state_summary || commitment_hash || memory_commitment)
```

---

*This document is the single source of truth for Opolys development. Update it with every design decision and implementation change.*