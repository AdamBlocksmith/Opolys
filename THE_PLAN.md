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
8. [Refiner Staking (PoS)](#8-refiner-staking-pos)
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
22. [Block & Transaction Validation](#22-block--transaction-validation)
23. [New Constants](#23-new-constants)
24. [Key Formulas Reference](#key-formulas-reference)
25. [Security Audit & Bug Tracker](#25-security-audit--bug-tracker)
26. [Implementation Plan — Pass 1](#26-implementation-plan--pass-1)

---

## 1. Vision & Philosophy

Opolys is **decentralized digital gold** — a pure coin with:

- **No tokens, no assets, no governance, no hardcoded caps**
- Every parameter emerges from mathematics or market forces
- Supply grows via block rewards (mirroring real gold production) and contracts via fee burning (mirroring gold attrition)
- Only double-signing gets slashed — 100% burn, no graduated penalties
- Refiners produce blocks when miners can't — no ConsensusPhase, no phase transitions
- Mining is opt-in via `--mine` flag, refining is opt-in via `--refine` flag
- Community builds explorers, wallets, mining pools — the core is the protocol layer (like Bitcoin Core)

**Tech direction**: EVO-OMAP PoW, BLS signatures, VRF, stealth addresses, viewing keys, Poseidon hash. NO WASM, NO object model, NO multi-asset, NO governance. The coin stays "just a coin" but with better privacy and decentralization primitives.

---

## 2. Currency Model

OPL uses 6 decimal places:

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
| `BASE_REWARD` | `332,000,000` Flakes (332 OPL) | Gold-derived block reward base |
| `MIN_DIFFICULTY` | `1` | Mathematical floor (not a cap) |
| `EPOCH` | `960` blocks (= exactly 24 hours at 90 s/block) | Unified epoch for retarget, dataset regen, unbonding |
| `UNBONDING_DELAY_BLOCKS` | `960` | One epoch delay for unbonding |
| `MIN_FEE` | `1` Flake | Floor for market-driven fees |
| `MIN_BOND_STAKE` | `1,000,000` Flakes (1 OPL) | Minimum per new bond entry |
| `BLOCK_VERSION` | `1` | Current block header version |
| `SIGNATURE_TYPE_ED25519` | `0` | ed25519 signature type constant |
| `EXTENSION_TYPE_NONE` | `0` | No extension data |
| `EXTENSION_TYPE_ROLLUP` | `1` | Rollup data (reserved) |
| `BLOCK_TARGET_TIME_MS` | `90,000` | 90 seconds per block |
| `BLOCK_TARGET_TIME_SECS` | `90` | 90 seconds per block |
| `MAX_ACTIVE_REFINERS` | `5,000` | Active refiner set cap |
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
| **Signing** | ed25519 (via ed25519-dalek) | Transaction auth and refiner block signing |
| **Key Derivation** | SLIP-0010 + HMAC-SHA512 | BIP-44 path: `m/44'/999'/0'/0'` |
| **Mnemonic** | BIP-39 (24-word, 256-bit entropy) | Wallet recovery |

### Planned (Pass 2+)

| Layer | Algorithm | Purpose |
|---|---|---|
| **Refiner Attestations** | ed25519 per-block signatures | Block confidence and attestation weight |
| **Signature Aggregation** | BLS12-381 | Efficient attestation aggregation (Pass 3+) |
| **Block Producer Selection** | VRF | Unpredictable, verifiable refiner selection |
| **Privacy (L1)** | Stealth addresses | Receiver privacy via one-time addresses |
| **Privacy (L2)** | Viewing keys | Selective transaction visibility |
| **ZK Foundation** | Poseidon hash | ZK-friendly hash for future SNARKs/STARKs |

### ObjectId

Account addresses are **Blake3-256 hashes of ed25519 public keys** — not the public keys themselves. This provides a 32-byte uniform address space and an extra hash layer.

---

## 5. Consensus Model

Opolys uses **hybrid PoW/PoS** with an **implicit transition** — no ConsensusPhase enum.

- **Miners** produce blocks by solving EVO-OMAP PoW puzzles
- **Refiners** produce blocks when no miner has produced one within the target interval (90 seconds)
- The reward split is **continuous** — `coverage_milli = (bonded_stake × 1000) / total_issued`
- When stake coverage is 0%, all rewards go to miners. At 100%, all go to refiners
- There is no phase switch, no threshold, no governance vote

### Refiner Block Production

Instead of a competing schedule, refiners produce blocks only when the chain is stalled:

```
forever:
  last_block_time = chain.block_timestamps.last()
  now = current_time()
  elapsed = now - last_block_time
  
  if elapsed < BLOCK_TARGET_TIME_MS (90,000):
    sleep(BLOCK_TARGET_TIME_MS - elapsed)
    continue
  
  // Target interval passed with no miner block
  producer = refiners.select_block_producer(timestamp, seed)
  if producer.object_id == self.miner_id:
    produce_refiner_block()
```

Key behavior:
- If a miner produces at time T, the refiner timer resets to T + 90s
- If no miner produces within 90s, the selected refiner produces
- If the selected refiner is offline, no block until the next interval
- The deterministic seed (`Blake3(prev_block_hash)[0..8]` as u64) ensures all nodes agree on which refiner should produce

### Block Mutual Exclusivity

A block must have **exactly one** of: PoW proof or refiner signature.

- PoW block: has `pow_proof`, no `refiner_signature`. Producer earns `pow_share` of block reward.
- Refiner block: has `refiner_signature`, no `pow_proof`. Producer earns `pow_share` of block reward (refiners get pos_share distributed among all active refiners).
- A block with both or neither is **rejected**.

---

## 6. Gold-Derived Emission

### Derivation

| Metric | Value | Source |
|---|---|---|
| Annual gold production | 3,630 tonnes | USGS/WGC 2024-2025 avg |
| Annual production in troy oz | ~116,707,041 | 3,630 × 32,150.7 |
| Blocks per year | 350,400 | 365.25 × 86,400 / 90 |
| **BASE_REWARD** | **332 OPL** | floor(116,707,041 ÷ 350,400) |

### Block Reward Formula

```
block_reward = (BASE_REWARD / difficulty) × vein_yield
```

Where:
- `BASE_REWARD` = 332 OPL (332,000,000 flakes) from genesis ceremony
- `difficulty` = effective difficulty (max of retarget, consensus_floor, MIN_DIFFICULTY)
- `vein_yield` = `1 + ln(target / hash_int)` (see Section 12)

### Natural Equilibrium

There is **no hard cap**. Issuance shrinks as difficulty rises (like gold getting harder to mine). Fee burning reduces supply. The two forces reach a natural equilibrium where market-driven fees balance new issuance.

---

## 7. Difficulty & Retargeting

### Genesis Difficulty

Genesis difficulty: 7
- At difficulty 7, single Ryzen 7 7700 parallel produces ~86.5s blocks (vs 90s target)
- First retarget at block 960 (~24 hours) self-corrects automatically

### Retarget Algorithm

Every `EPOCH` (960 blocks = exactly 24 hours):

```
new_difficulty = old_difficulty × (expected_time / actual_time)
```

If blocks were too fast (actual < expected), difficulty increases. If too slow (actual > expected), difficulty decreases.

**No maximum clamp.** The only floor is `MIN_DIFFICULTY` (1), which is a mathematical requirement since difficulty 0 would make all hashes valid.

### Consensus Floor

```
consensus_floor = total_issued / bonded_stake
```

When `bonded_stake = 0`, floor = 0 (no refiners yet).

### Effective Difficulty

```
effective_difficulty = max(retarget, consensus_floor, MIN_DIFFICULTY)
```

---

## 8. Refiner Staking (PoS)

### Bond Lifecycle

1. `RefinerBond { amount }` — Lock `amount` OPL as stake. Creates a new entry if the refiner already exists (top-up).
2. `RefinerUnbond { amount }` — Withdraw `amount` OPL using **FIFO order** (see Section 9).
3. **Slashing** — Only for double-signing. **100% of stake burned** on any double-sign offense. No graduated penalties, no offense counter, no reset window. Slashed stake is removed from circulation, not confiscated to any treasury.

### Per-Entry Weight

Each `BondEntry` has its own seniority clock:

```
entry_weight = entry.stake × (1 + ln(1 + entry.age_years))
```

Logarithmic seniority means older entries earn more per-coin, but the marginal gain diminishes — preventing permanent dominance by early stakers.

### PoS vs PoW Block Reward

```
PoS block reward = BASE_REWARD / difficulty × 1.0 (flat, no vein yield)
PoW block reward = BASE_REWARD / difficulty × vein_yield (1.0x to ~10x)
```

Refiners earn steady predictable income (like gold vaults), miners earn variable income based on luck (like gold miners). PoS blocks pass `hash_int = 0` to `compute_vein_yield()`, which returns the 1.0x floor by design.

### Block Producer Selection

Weighted random sampling from active refiners. The seed is derived from the first 8 bytes of the previous block hash (`Blake3(prev_block_hash)[0..8]` as `u64`), making selection deterministic and verifiable.

### Minimum Bond

New bond entries require at least `MIN_BOND_STAKE` (1 OPL). Residuals from FIFO splitting are exempt from this minimum.

### Refiner Activation

Newly bonded refiners start in `Bonding` status. They activate to `Active` once their earliest bond entry has been confirmed for at least one full epoch (960 blocks) and the active set has a free slot. Checked every block via `activate_matured_refiners()` in `apply_block`.

**Maximum active refiners: 5,000 (launch cap)**
- `MAX_ACTIVE_REFINERS = 5,000` in `constants.rs`
- New refiners bond successfully and wait in `Bonding` status
- Promoted when a slot opens (via unbond or slash)
- No `RefinerBond` transaction is ever rejected — all are queued fairly

### Attestations (Pass 2 — Not Yet Implemented)

Refiners sign block hashes using ed25519. Attestations are collected by the next block's producer and included in that block. Reliability is tracked as `consecutive_correct_attestations` per refiner:

```
reliability = 1 + ln(1 + consecutive_correct / EPOCH)
attestation_weight = stake × seniority × reliability
```

Refiners who miss a block (were online but didn't attest) have their reliability reset to 0. Block confidence is derived on-chain from attestation weight vs total bonded stake.

---

## 9. FIFO Unbonding

### `RefinerUnbond { amount: FlakeAmount }`

Withdraws `amount` OPL using **FIFO order** — oldest entries consumed first:

1. Sort entries by `bonded_at_timestamp` (oldest first)
2. Consume entries from the front:
   - If `entry.stake <= remaining_amount`: consume entire entry
   - If `entry.stake > remaining_amount`: **split** the entry
     - Return `remaining_amount` to sender
     - Keep residual with **original timestamp** (preserves seniority)
3. Residuals keep their original `bonded_at_timestamp`
4. Auto-merge entries with the same `bonded_at_timestamp`

Unbonded stake enters the **unbonding queue** — a list of `PendingUnbond` entries. After `UNBONDING_DELAY_BLOCKS` (960 blocks = exactly 24 hours), matured entries are automatically credited back to the sender's account.

---

## 10. Fees & Burning

All transaction fees are **permanently burned** — not collected by refiners or miners.

- **Suggested fee**: `suggested_fee` field in `BlockHeader`, computed via EMA of previous block's fees. Starts at `MIN_FEE` (1 Flake).
- **No minimum fee beyond 1 Flake**: Market determines inclusion
- **Refiner income**: Block rewards only, not fees
- **Deflationary**: Fee burning reduces circulating supply
- **Suggested fee uses burned fees, not declared fees**: Computed from `total_fees_burned` after transaction execution, not `total_fees` before execution. This prevents failed transactions from inflating the fee market signal.

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
| `EPOCH_LENGTH` | 960 | Blocks per dataset epoch (matches EPOCH constant) |

---

## 12. Vein Yield

Vein yield replaces discovery bonus with a mathematically cleaner formula:

```
vein_yield = 1 + ln(target / hash_int)
```

Where:
- `target = 2^(64-D) - 1` for difficulty D (leading zero bits model)
- `hash_int` = first 8 bytes of the EVO-OMAP PoW hash, interpreted as big-endian u64

This gives a natural distribution: most blocks earn ~2x BASE_REWARD, exceptionally lucky blocks earn more. The expected value is approximately 2.0.

Implementation uses `f64::ln()` with deterministic rounding via `ln_milli()`. IEEE 754 guarantees identical results across all platforms.

---

## 13. Block Structure

```rust
BlockHeader {
    version: u32,                          // Protocol version (currently 1)
    height: u64,                           // 0 for genesis
    previous_hash: Hash,                   // Blake3-256 of prior block
    state_root: Hash,                      // Blake3-256 of post-execution state
    transaction_root: Hash,                // Blake3-256 commitment of tx IDs
    timestamp: u64,                         // UNIX epoch seconds
    difficulty: u64,                        // Effective difficulty
    suggested_fee: FlakeAmount,             // EMA of previous block's fees
    extension_root: Option<Hash>,           // Reserved for rollups
    producer: ObjectId,                     // Block producer (miner or refiner)
    pow_proof: Option<Vec<u8>>,             // EVO-OMAP nonce (None for refiner blocks)
    refiner_signature: Option<Vec<u8>>,     // ed25519 signature (None for PoW blocks)
}

Block {
    header: BlockHeader,
    transactions: Vec<Transaction>,
    slash_evidence: Vec<DoubleSignEvidence>,
    genesis_ceremony: Option<GenesisCeremonyData>,
}
```

### Block Hash

`Blake3-256(header_bytes)` where `pow_proof` and `refiner_signature` are set to `None` before hashing. The `version`, `suggested_fee`, `extension_root`, and `producer` fields are included in the hash.

### Mutual Exclusivity

A block must have **exactly one** of `pow_proof` or `refiner_signature`:
- **PoW block**: `pow_proof = Some(...)`, `refiner_signature = None`
- **Refiner block**: `refiner_signature = Some(...)`, `pow_proof = None`
- **Genesis block** (height 0): both `None`
- **Invalid**: both `Some(...)`, or both `None` at height > 0

---

## 14. Transaction Model

### Types

```rust
enum TransactionAction {
    Transfer { recipient: ObjectId, amount: FlakeAmount },
    RefinerBond { amount: FlakeAmount },
    RefinerUnbond { amount: FlakeAmount },  // FIFO unbonding
}
```

### Transaction Structure

```rust
Transaction {
    tx_id: ObjectId,           // Blake3-256(sender_hex || borsh(action) || fee || nonce || chain_id)
    sender: ObjectId,          // Blake3-256(ed25519_pubkey)
    action: TransactionAction,
    fee: FlakeAmount,           // Burned, not collected
    signature: Vec<u8>,        // ed25519 signature over Borsh(sender, action, fee, nonce, chain_id)
    signature_type: u8,         // 0 = ed25519 (reserved for post-quantum)
    nonce: u64,                 // Replay protection
    chain_id: u64,              // Cross-chain replay protection
    data: Vec<u8>,             // Arbitrary attachment
    public_key: Vec<u8>,        // ed25519 verifying key (32 bytes)
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
| `opl_getRefiners` | _(none)_ | Refiner set with FIFO bond entries |
| `opl_getBlockConfidence` | _(none)_ | Block confidence score (Pass 2) |

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

SLIP-0044 coin type 999 for OPL. Single ed25519 key handles both transaction signing and refiner block signing. Full wallet recovery from mnemonic alone.

### Security

- Mnemonic read from `OPOLYS_MNEMONIC` env var or stdin prompt (never CLI args)
- Key material zeroed on drop using `zeroize` crate
- Manual `Debug` impls that redact secrets
- Key files written with mode 0600

---

## 17. Networking (libp2p)

- **Transport**: libp2p 0.54 with QUIC, TCP, noise, yamux, relay client
- **Discovery**: Kademlia DHT (bucket size 20) + identify protocol
- **Gossip**: Gossipsub for block/transaction propagation (`opolys/tx/v1`, `opolys/block/v1`)
- **Sync**: CBOR request-response protocol for block download (`/opolys/sync/1`)
- **Attestation** (Pass 2): `opolys/attestation/v1` gossipsub topic
- **Ping**: Liveness checks with 30s interval, 10s timeout
- **Challenge**: Memory-fingerprinting challenge before accepting gossip

---

## 18. Storage

Opolys uses **RocksDB** with **Borsh** serialization. State is saved after each block. Tests use `tempfile::tempdir()` for isolation.

### Pass 1 Storage Fixes

- Atomic writes across column families (batch writes)
- BLAKE3 integrity checksums on stored values
- WAL sync mode enabled
- Index and block data saved atomically

---

## 19. Architecture & Crate Map

```
Opolys/
├── Cargo.toml                         # Workspace
├── crates/
│   ├── core/src/
│   │   ├── constants.rs                # EPOCH, MIN_FEE, MIN_BOND_STAKE, BLOCK_VERSION, etc.
│   │   ├── types.rs                    # Hash, ObjectId, BlockHeader, RefinerStatus, RefinerBond, RefinerUnbond
│   │   └── errors.rs                   # OpolysError enum
│   ├── crypto/src/
│   │   ├── hash.rs                     # Blake3-256 (with domain separation), SHA3-256, Blake3 XOF
│   │   └── signing.rs                  # ed25519 verification (with domain separation)
│   ├── consensus/src/
│   │   ├── account.rs                  # AccountStore with fee-burning transfers
│   │   ├── block.rs                    # compute_block_hash(), compute_transaction_root(), validate_block()
│   │   ├── difficulty.rs               # Retarget (expected/actual), consensus floor, no clamp
│   │   ├── emission.rs                 # Vein yield, ln_milli (f64), suggested_fee EMA, refiner weight
│   │   ├── mempool.rs                  # Fee-priority mempool with min fee, nonce gap, expiry
│   │   ├── refiner.rs                  # RefinerSet with FIFO unbonding, 100% slash
│   │   ├── pow.rs                      # EVO-OMAP PowContext, verify, compute_pow_hash_value
│   │   └── genesis.rs                  # Genesis block construction
│   ├── storage/src/store.rs            # RocksDB persistence (atomic writes, checksums)
│   ├── execution/src/dispatcher.rs      # TransactionDispatcher (Transfer, Bond, Unbond FIFO)
│   ├── wallet/src/
│   │   ├── bip39.rs                    # BIP-39 + SLIP-0010 derivation (mnemonic from env/stdin)
│   │   ├── signing.rs                  # TransactionSigner with signature_type
│   │   ├── key.rs                      # KeyPair (with zeroize, manual Debug)
│   │   └── account.rs                  # AccountInfo
│   ├── rpc/src/server.rs              # JSON-RPC 2.0 with auth, CORS localhost, body size limit
│   ├── networking/src/
│   │   ├── behaviour.rs              # OpolysBehaviour (gossipsub+kad+identify+ping+request_response)
│   │   ├── network.rs               # OpolysNetwork, persistent keypair, response size limits
│   │   ├── gossip.rs                # GossipConfig (tx/block/attestation topics, max message = block max)
│   │   ├── sync.rs                  # SyncRequest, SyncResponse (CBOR), SyncConfig
│   │   ├── challenge.rs             # Memory fingerprinting challenges bound to PeerId
│   │   └── discovery.rs            # DiscoveryConfig (Kad bucket size)
│   └── node/src/
│       ├── main.rs                   # CLI (--mine, --refine, --key-file), P2P event loop
│       └── node.rs                   # OpolysNode with hybrid PoW/refiner loop, apply_block
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
| `rayon` | 1.10 | Parallel mining |
| `zeroize` | TBD | Key material zeroing |
| `subtle` | TBD | Constant-time comparisons |

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
| 7: RPC | **DONE** | JSON-RPC 2.0 with mining endpoints, API key auth |
| 8: Node | **DONE** | Full node with mining loop and block application |
| 9: Networking | **DONE** | P2P gossip/sync/discovery wired to node |
| 10: Staking | **DONE** | `--validate`, graduated slash (→ 100% slash in Pass 1), PoS block production |
| 11: Security | **DONE** | Eclipse protection, subnet diversity, DoS limits, memory challenge |
| 12: Pass 1 | **IN PROGRESS** | Refiner rename, 100% slash, hybrid fix, security fixes |
| 13: Pass 2 | **PLANNED** | Attestations, reliability score, block confidence |
| 14: Mainnet | **READY** | Genesis ceremony and launch (after Pass 1+2) |

---

## 21. Test Count

**Full test suite passing** across all crates. Run with `cargo test --workspace`.

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
12. **Mutual exclusivity**: Block must have exactly one of `pow_proof` or `refiner_signature` (genesis exempt)
13. **PoW proof**: For PoW blocks, EVO-OMAP proof must satisfy the difficulty target
14. **Refiner signature**: For refiner blocks, signature must verify and producer must be the selected refiner

### Transaction Verification (`verify_transaction`)

1. **chain_id**: Must match `MAINNET_CHAIN_ID` (prevents cross-chain replay)
2. **tx_id integrity**: Recomputed from (sender, action, fee, nonce, chain_id) must match declared `tx_id`
3. **signature_type**: Must be `SIGNATURE_TYPE_ED25519` (0)
4. **ed25519 signature**: Verified against stored public key. `Blake3(public_key) == sender` must hold.

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
suggested_fee = EMA(total_fees_burned, previous_suggested_fee)
              = (burned + 9 × old) / 10, floored at MIN_FEE
```

### Refiner Weight
```
entry_weight = entry.stake × (1 + ln(1 + entry.age_years))
```

### Stake Coverage
```
coverage_milli = (bonded_stake × 1000) / total_issued   // integer, no float
pow_share_amount = block_reward × (1000 - coverage_milli) / 1000
pos_share_amount = block_reward - pow_share_amount
```
PoW share goes to the block producer. PoS share is distributed among active refiners proportional to weight.

### EVO-OMAP PoW Verification
```
target = 2^(64-D) - 1    // D = difficulty (leading zero bits)
valid if: u64(pow_hash[..8]) <= target
pow_hash = SHA3-256(state_summary || commitment_hash || memory_commitment)
```
EVO-OMAP difficulty means **leading zero bits** in the SHA3-256 output, NOT a u64 divisor.

---

## 25. Security Audit & Bug Tracker

### CRITICAL (4)

| # | Issue | Location | Status |
|---|---|---|---|
| C1 | Testnet/mainnet shared data directory | `node.rs:433-498` | **OPEN** |
| C2 | Mnemonic passed as CLI argument | `wallet/main.rs` | **OPEN** |
| C3 | No memory zeroing for private keys | `wallet/bip39.rs:47, key.rs:60` | **OPEN** |
| C4 | RPC write/mining endpoints unauthenticated by default | `rpc/server.rs:169-175` | **OPEN** |

### HIGH (9)

| # | Issue | Location | Status |
|---|---|---|---|
| H1 | No hash domain separation | `crypto/hash.rs`, multiple | **OPEN** |
| H2 | No signing domain separation | `wallet/signing.rs:46, node.rs:737` | **OPEN** |
| H3 | Suggested fee from declared fees, not burned | `node.rs:1022` | **OPEN** |
| H4 | Unbond fee bypass — fee skipped if balance < fee | `dispatcher.rs:270-284` | **OPEN** |
| H5 | RocksDB non-atomic cross-CF writes | `store.rs:103,110` | **OPEN** |
| H6 | No sync response size limit | `network.rs:204-207` | **OPEN** |
| H7 | request_timeout_secs never applied | `network.rs:204-207` | **OPEN** |
| H8 | Block indexes saved separately from block data | `store.rs:155 vs 95` | **OPEN** |
| H9 | No integrity checksums on persisted data | `store.rs` throughout | **OPEN** |

### HIGH — P2P & Storage (included above)

### MEDIUM (18 — 5 already fixed)

| # | Issue | Location | Status |
|---|---|---|---|
| M1 | Graduated slashing contradicts THE_PLAN (should be 100% burn) | `pos.rs:500-553` | **OPEN** |
| M2 | Slash reset window 10,240 blocks undocumented | `pos.rs:518-520` | **OPEN** |
| M3 | PoS sig verification runs on PoW blocks | `node.rs:845-871` | **OPEN** |
| M4 | PoW/PoS mutual exclusivity not checked | `block.rs:270-308` | **OPEN** |
| M5 | Producer not validated for PoW blocks | `node.rs:991-997` | **OPEN** |
| M6 | Zero-miner-id phantom issuance | `node.rs:991 vs 1043` | **OPEN** |
| M7 | API key timing attack | `rpc/server.rs:178-190` | **OPEN** |
| M8 | CORS allows all origins | `rpc/server.rs:903-907` | **OPEN** |
| M9 | No request body size limit | `rpc/server.rs:486-489` | **OPEN** |
| M10 | Key file world-readable | `wallet/key.rs:192` | **OPEN** |
| ~~M11~~ | ~~Mempool expiry not enforced~~ | `mempool.rs` | **FIXED** |
| ~~M12~~ | ~~Mempool doesn't check minimum fee~~ | `mempool.rs` | **FIXED** |
| M13 | Ephemeral P2P keypair | `network.rs:167` | **OPEN** |
| M14 | TOCTOU race — height check vs apply_block | `main.rs:683 vs 738` | **OPEN** |
| M15 | Sync start_height unvalidated | `main.rs:1009` | **OPEN** |
| M16 | No WAL sync mode on RocksDB | `store.rs:74` | **OPEN** |
| ~~M17~~ | difficulty_to_target(1) = 2^63-1 | `emission.rs:37-42` | **BY DESIGN** |
| M18 | Integer division bias in retarget | `difficulty.rs:133` | **OPEN** |
| M19 | Dead code: compute_pow_share/compute_pos_share use f64 | `emission.rs:174-193` | **OPEN** |
| M20 | chain.base_reward always BASE_REWARD | Throughout | **BY DESIGN** |
| M21 | Silent Borsh error handling in state root | `account.rs:219, pos.rs:635-648` | **OPEN** |
| M22 | apply_bond refund discards errors | `dispatcher.rs:217` | **OPEN** |
| M23 | Wallet HTTP default | `wallet/main.rs:22` | **OPEN** |

### MEDIUM — Calculations (7)

| # | Issue | Location | Status |
|---|---|---|---|
| ~~M17~~ | ~~difficulty_to_target oddity~~ | `emission.rs:37-42` | **BY DESIGN** |
| M18 | Integer division bias in retarget | `difficulty.rs:133` | **OPEN** |
| ~~M19~~ | ~~Dead f64 share functions~~ | `emission.rs:174-193` | **OPEN** |
| ~~M20~~ | ~~chain.base_reward always BASE_REWARD~~ | Throughout | **BY DESIGN** |
| M21 | Silent Borsh error handling | `account.rs, pos.rs` | **OPEN** |

### LOW (9)

| # | Issue | Location | Status |
|---|---|---|---|
| L1 | Debug derives on Bip39Mnemonic and KeyPair | `bip39.rs:39, key.rs:60` | **OPEN** |
| L2 | Mnemonic printed to stdout on opl new | `wallet/main.rs:168-169` | **OPEN** |
| L3 | Early-return timing in verify_ed25519 | `crypto/signing.rs:57-74` | **OPEN** |
| L4 | Dual serialization for tx_id vs signed data | `wallet/signing.rs:46,138-151` | **OPEN** |
| L5 | Hex-encoded sender in tx_id | `wallet/signing.rs:145` | **OPEN** |
| L6 | No SLIP-0010 reference test vectors | `wallet/bip39.rs:199-277` | **OPEN** |
| L7 | Gossip max message (5 MiB) vs block max (10 MiB) | `gossip.rs:27 vs constants.rs:207` | **OPEN** |
| L8 | Challenge protocol doesn't bind to PeerId | `challenge.rs` | **OPEN** |
| L9 | Bip39Mnemonic::generate() panics on entropy failure | `wallet/bip39.rs:47-48` | **OPEN** |

---

## 26. Implementation Plan — Pass 1

### Phase A: Rename (touches everything, do first)

1. Full validator → refiner rename across all crates, RPC, CLI, docs
2. `ConsensusPhase` enum → **delete entirely** (implicit phase)
3. `--validate` → `--refine` CLI flag
4. `validator_signature` → `refiner_signature`
5. `ValidatorBond/Unbond` → `RefinerBond/Unbond`
6. `ValidatorInfo/Set/Status` → `RefinerInfo/Set/Status`
7. `graduated_slash()` → `slash_refiner()` (100% burn)
8. Delete `slash_offense_count` and `last_slash_height` from `RefinerInfo`
9. `pos.rs` → `refiner.rs`
10. All variable names: `validators` → `refiners`, `validator_id` → `refiner_id`, etc.
11. Update all tests with refiner terminology
12. Verify all tests pass after rename

### Phase B: Security Fixes (CRITICAL)

13. C1: Testnet/mainnet data directory isolation + chain_id check on load
14. C2: Mnemonic from `OPOLYS_MNEMONIC` env var or stdin prompt (using `rpassword`)
15. C3: `zeroize` crate on `DerivedSeed`/`KeyPair`; manual `Debug` impls that redact
16. C4+M7: RPC API key defaults to random-generated (printed at startup); constant-time comparison via `subtle::ConstantTimeEq`

### Phase C: Protocol Fixes (consensus behavior changes)

17. Delete `ConsensusPhase` from `ChainState` and `PersistedChainState`
18. Refiner loop: produce after `BLOCK_TARGET_TIME_MS` (90,000ms) with no miner block
19. M3/M4: Mutual exclusivity check in `validate_block()`
20. M1/M2: Replace `graduated_slash` with 100% burn on any double-sign
21. M6: Zero-miner-id — skip PoW share crediting when `miner_id` is zero
22. H4: Unbond fee — reject if `balance < fee`, don't skip burn
23. H3: Suggested fee — compute from `total_fees_burned`, not `total_fees`
24. M5: Validate producer field on PoW blocks (reject zero-id producer)

### Phase D: Storage & P2P Fixes

25. H5/H8: Atomic RocksDB writes (batch across column families)
26. H6: Sync response size limit in `request_response::Config`
27. H7: Wire `request_timeout_secs` to libp2p config
28. H9: BLAKE3 integrity checksums on stored values, verify on load
29. M16: WAL sync mode on RocksDB
30. M13: Persistent P2P keypair from data dir
31. M15: Validate `start_height` in sync requests, clamp to `[0, chain.height]`

### Phase E: Medium Fixes

32. M8: CORS restrict to localhost origins only
33. M9: Request body size limit (1 MiB default)
34. M10: Key file permissions `chmod 600`
35. M21: Silent Borsh errors → `expect()` instead of `if let Ok()`
36. M22: Propagate `apply_bond` credit errors
37. M23: Wallet RPC default to `https://`
38. M14: Acquire write lock before height check (fix TOCTOU)
39. M19: Delete dead `compute_pow_share`/`compute_pos_share` f64 functions from `emission.rs`

### Phase F: Low / Cleanup

40. L3: Constant-time `verify_ed25519` via `subtle`
41. L4/L5: Unify tx_id serialization (deferred — breaking change)
42. L6: Add SLIP-0010 reference test vectors
43. L7: Raise gossip max message to match `MAX_BLOCK_SIZE_BYTES`
44. L8: Challenge protocol bind to PeerId
45. L9: `Bip39Mnemonic::generate()` → return `Result`

### Phase G: Pass 2 (After Pass 1 is tested and working)

46. Attestation struct and `opolys/attestation/v1` P2P topic
47. Attestation collection in block builder
48. Attestation verification in `apply_block`
49. Reliability score: `consecutive_correct_attestations` in `RefinerInfo`
50. Attestation weight in reward distribution
51. Block confidence score derived on-chain
52. `opl_getBlockConfidence` RPC endpoint

---

*This document is the single source of truth for Opolys development. Update it with every design decision and implementation change.*