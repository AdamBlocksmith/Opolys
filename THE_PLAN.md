# THE PLAN — Opolys ($OPL) & Eco-OMAP

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
9. [Per-Entry Validator Bonds](#9-per-entry-validator-bonds)
10. [FIFO Unbonding (Planned)](#10-fifo-unbonding-planned)
11. [Stake-Weighted Mean Age (Planned)](#11-stake-weighted-mean-age-planned)
12. [Fees & Burning](#12-fees--burning)
13. [Block Structure](#13-block-structure)
14. [Transaction Model](#14-transaction-model)
15. [RPC API](#15-rpc-api)
16. [Wallet Key Derivation](#16-wallet-key-derivation)
17. [OMAP Proof-of-Work (Planned)](#17-omap-proof-of-work-planned)
18. [BLS12-381 Signatures (Planned)](#18-bls12-381-signatures-planned)
19. [VRF Validator Selection (Planned)](#19-vrf-validator-selection-planned)
20. [Stealth Addresses (Planned)](#20-stealth-addresses-planned)
21. [Viewing Keys (Planned)](#21-viewing-keys-planned)
22. [Poseidon Hash (Planned)](#22-poseidon-hash-planned)
23. [Networking (libp2p)](#23-networking-libp2p)
24. [Storage](#24-storage)
25. [Architecture & Crate Map](#25-architecture--crate-map)
26. [Known Bugs & Fixes](#26-known-bugs--fixes)
27. [Build Sequence](#27-build-sequence)
28. [Test Count](#28-test-count)

---

## 1. Vision & Philosophy

Opolys is **decentralized digital gold** — a pure coin with:

- **No tokens, no assets, no governance, no hardcoded caps**
- Every parameter emerges from mathematics or market forces
- Supply grows via block rewards (mirroring real gold production) and contracts via fee burning (mirroring gold attrition)
- Only double-signing gets slashed — no reversal windows, no confiscation
- Mining is opt-in via `--mine` flag, not default
- Community builds explorers, wallets, mining pools — the core is the protocol layer (like Bitcoin Core)

**Tech direction**: OMAP PoW, BLS signatures, VRF, stealth addresses, viewing keys, Poseidon hash. NO WASM, NO object model, NO multi-asset, NO governance. The coin stays "just a coin" but with better privacy and decentralization primitives.

---

## 2. Currency Model

OPL uses 6 decimal places, named after real gold weight units:

| Unit | Flakes | OPL | Example |
|---|---|---|---|
| **OPL** | 1,000,000 | 1 | `1.000000 OPL` |
| **Pennyweight (dwt)** | 10,000 | 0.01 | `0.010000 OPL` |
| **Grain (gr)** | 100 | 0.0001 | `0.000100 OPL` |
| **Flake** | 1 | 0.000001 | `0.000001 OPL` |

All on-chain arithmetic uses `FlakeAmount` (u64). No floating-point in consensus logic.

---

## 3. All Constants

From `crates/core/src/constants.rs`:

| Constant | Value | Description |
|---|---|---|
| `CURRENCY_NAME` | `"Opolys"` | Human-readable name |
| `CURRENCY_TICKER` | `"OPL"` | Exchange ticker |
| `CURRENCY_SMALLEST_UNIT` | `"Flake"` | Name of 1/1,000,000 OPL |
| `FLAKES_PER_OPL` | `1_000_000` | Fundamental unit ratio |
| `PENNYWEIGHTS_PER_OPL` | `100` | 1 dwt = 0.01 OPL |
| `GRAINS_PER_OPL` | `10_000` | 1 gr = 0.0001 OPL (100 Flakes each) |
| `DECIMAL_PLACES` | `6` | Always 6 decimal places |
| `BASE_REWARD` | `440,000,000` Flakes (440 OPL) | Gold-derived block reward |
| `MIN_DIFFICULTY` | `1` | Floor difficulty |
| `RETARGET_EPOCH` | `1,024` blocks | Difficulty adjustment interval |
| `POS_FINALITY_BLOCKS` | `3` | PoS finality depth |
| `BLOCK_TARGET_TIME_SECS` | `120` | One block every 2 minutes |
| `MIN_BOND_STAKE` | `100,000,000` Flakes (100 OPL) | Minimum per-entry validator bond |
| `BLOCK_CAPACITY_RATE` | `10,000` | Bytes per second of target block time |
| `block_max_capacity_bytes()` | `1,200,000` | Max block size (120 × 10,000) |
| `MAX_INBOUND_CONNECTIONS` | `50` | Inbound peer connections |
| `MAX_OUTBOUND_CONNECTIONS` | `50` | Outbound peer connections |
| `MAX_PEER_COUNT` | `200` | Peer manager capacity |
| `SYNC_MAX_BLOCKS_PER_REQUEST` | `500` | Max blocks per sync request |
| `SYNC_MAX_HEADERS_PER_REQUEST` | `2,000` | Max headers per sync request |
| `SYNC_REQUEST_TIMEOUT_SECS` | `30` | Sync request timeout |
| `SYNC_PARALLEL_PEER_COUNT` | `3` | Parallel sync peers |
| `KAD_BUCKET_SIZE` | `20` | Kademlia DHT bucket size |
| `KAD_QUERY_TIMEOUT_SECS` | `60` | DHT query timeout |
| `PING_INTERVAL_SECS` | `30` | Peer liveness ping interval |
| `PING_TIMEOUT_SECS` | `20` | Ping response timeout |
| `DEFAULT_LISTEN_PORT` | `4170` | P2P listen port |
| `GOSSIP_MAX_MESSAGE_SIZE_BYTES` | `5,242,880` (5 MiB) | Max gossip message size |
| `NETWORK_PROTOCOL_VERSION` | `"1.0.0"` | Protocol version string |
| `MEMPOOL_MAX_SIZE_BYTES` | `100,000,000` (100 MiB) | Max mempool memory |
| `MEMPOOL_MAX_TXS_PER_ACCOUNT` | `50` | Pending txs per account |
| `MEMPOOL_TX_EXPIRY_SECS` | `86,400` (24h) | Mempool tx expiry |
| `TX_MAX_SIZE_BYTES` | `100,000` | Max serialized tx size |

---

## 4. Cryptographic Stack

### Current (Implemented)

| Layer | Algorithm | Purpose |
|---|---|---|
| **Hashing** | Blake3-256 (32 bytes) | Block hashes, transaction IDs, ObjectIds, state roots, Merkle roots |
| **Signing** | ed25519 (via ed25519-dalek) | Transaction auth and validator block signing |
| **Key Derivation** | SLIP-0010 + HMAC-SHA512 | BIP-44 path: `m/44'/999'/0'/0'` |
| **Mnemonic** | BIP-39 (24-word, 256-bit entropy) | Wallet recovery |

### Planned (Not Yet Implemented)

| Layer | Algorithm | Purpose |
|---|---|---|
| **PoW** | OMAP (replacing Autolykos) | Memory-hard, read+write, ASIC-resistant mining |
| **Validator Signatures** | BLS12-381 | Signature aggregation for efficient PoS attestation |
| **Block Producer Selection** | VRF | Unpredictable, verifiable validator selection |
| **Privacy (L1)** | Stealth addresses | Receiver privacy via one-time addresses |
| **Privacy (L2)** | Viewing keys | Selective transaction visibility |
| **ZK Foundation** | Poseidon hash | ZK-friendly hash for future SNARKs/STARKs |

### ObjectId

Account addresses are **Blake3-256 hashes of ed25519 public keys** — not the public keys themselves. This provides a 32-byte uniform address space and an extra hash layer.

### Key Derivation

A single ed25519 keypair — derived deterministically from BIP-39 mnemonic via SLIP-0010 — handles both transaction signing and validator block signing. Full wallet recovery from mnemonic alone.

---

## 5. Consensus Model

Opolys uses **hybrid PoW/PoS** with a smooth transition:

- **PoW blocks** are mined when difficulty is high relative to stake coverage
- **PoS blocks** are produced by validators as stake coverage grows
- The split is **continuous** — no thresholds, no governance votes

### Consensus Phase Selection

The phase for each block is determined by `compute_stake_coverage(total_bonded, total_issued)`:

- `stake_coverage = total_bonded / total_issued`, clamped to [0.0, 1.0]
- `pow_share = 1.0 - stake_coverage` (miner reward fraction)
- `pos_share = stake_coverage` (validator reward fraction)

At 0% coverage: all rewards go to miners. At 100% coverage: all rewards go to validators. No sharp transitions.

---

## 6. Gold-Derived Emission

### Derivation

| Metric | Value | Source |
|---|---|---|
| Total above-ground gold | 219,891 tonnes | WGC, end-2025 |
| Annual gold production | 3,630 tonnes | USGS/WGC 2024-2025 avg |
| Annual production in troy oz | ~116,707,041 | 3,630 × 32,150.7 |
| Blocks per year | 262,980 | 365.25 × 24 × 60 × 60 / 120 |
| **BASE_REWARD** | **440 OPL** | floor(116,707,041 / 262,980) |

### Block Reward Formula

```
block_reward = BASE_REWARD / effective_difficulty × discovery_bonus
```

Where:
- `BASE_REWARD = 440 × FLAKES_PER_OPL = 440,000,000 flakes`
- `effective_difficulty = max(retarget, consensus_floor, MIN_DIFFICULTY)`
- `discovery_bonus ≈ √(MAX / (difficulty × hash))` — sub-linear reward for lucky miners

### Natural Equilibrium

There is **no hard cap**. Instead:
- Issuance: `BASE_REWARD / difficulty` — shrinks as difficulty rises (like gold getting harder to mine)
- Burning: All transaction fees are permanently destroyed
- Result: Circulating supply can decrease when fee burning exceeds new issuance

---

## 7. Difficulty & Retargeting

### Retarget Algorithm

Every `RETARGET_EPOCH` (1,024) blocks:

```
new_difficulty = old_difficulty × (actual_time / expected_time)
new_difficulty = clamp(new_difficulty, old/4, old×4)
new_difficulty = max(new_difficulty, MIN_DIFFICULTY)
```

### Consensus Floor

```
consensus_floor = total_issued / bonded_stake
```

When `bonded_stake = 0`, floor = 0 (no validators yet).

### Effective Difficulty

```
effective_difficulty = max(retarget, consensus_floor, MIN_DIFFICULTY)
```

### Discovery Bonus

```
ratio = u64::MAX / (difficulty × hash_value)
bonus = max(1, floor(√ratio))
```

Sub-linear scaling prevents inflation spikes while rewarding miners who find exceptionally good hashes.

---

## 8. Validator Staking (PoS)

### Bond Lifecycle

1. `ValidatorBond { amount }` — Lock `amount` OPL as stake. Creates a new entry if the validator already exists (top-up), or creates a new validator with their first entry.
2. `ValidatorUnbond { bond_id }` — Withdraw a specific entry's stake. If no entries remain, the validator is removed from the set entirely. *(Will change to FIFO `{ amount }` — see Section 10.)*
3. **Slashing** — Only for double-signing. All entries' stakes are **burned** (not confiscated). Validator status set to `Slashed`.

### Per-Entry Weight (Current Implementation)

Each `BondEntry` has its own seniority clock:

```
entry_weight = entry.stake × (1 + ln(1 + entry.age_years))
```

Total validator weight = sum of all entry weights. Logarithmic seniority means:
- At age 0: weight = stake × 1.0 (no bonus)
- At age 1 year: weight = stake × 1.693
- At age 5 years: weight = stake × 2.792
- The marginal gain diminishes — early validators earn more per-coin but never dominate permanently

### Block Producer Selection

Weighted random sampling:

```
target = seed % total_weight
cumulative = 0
for validator in active_validators:
    cumulative += validator.weight(current_timestamp)
    if cumulative > target:
        select(validator)
```

Seed is derived from on-chain entropy. No rounds, no schedules, no fixed validator sets.

### Minimum Bond

Each bond entry must be at least `MIN_BOND_STAKE` (100 OPL). Top-up entries also must meet this minimum (residuals after FIFO splitting are exempt).

---

## 9. Per-Entry Validator Bonds

### Current Implementation

```rust
pub struct BondEntry {
    pub bond_id: u64,              // Auto-incrementing per-validator counter
    pub stake: FlakeAmount,         // OPL locked in this entry
    pub bonded_at_height: u64,      // Block height when bonded
    pub bonded_at_timestamp: u64,   // Unix timestamp for seniority calculation
}
```

Each validator holds a `Vec<BondEntry>`. Top-up bonding creates a new entry with `bond_id = next_bond_id++`. Unbonding targets a specific `bond_id`.

### Error Handling

- **Invalid bond_id**: Transaction fails with no fee burn and no nonce advance. Honest mistakes shouldn't cost money.
- **Insufficient balance for bond + fee**: Transaction fails, no state change.
- **Last entry unbonded**: Validator removed from set entirely.

### Pools Are Off-Chain

The protocol provides per-entry bonds. Community builds pooling solutions. No pool primitives in the protocol.

---

## 10. FIFO Unbonding (Planned)

### Current: `ValidatorUnbond { bond_id: u64 }`

Targets a specific entry by ID. Requires knowing your bond_ids.

### Planned: `ValidatorUnbond { amount: FlakeAmount }`

Withdraws `amount` OPL using **FIFO order** — oldest entries consumed first.

#### FIFO Logic

1. Sort entries by `bonded_at_timestamp` (oldest first)
2. Consume entries from the front:
   - If `entry.stake <= remaining_amount`: consume entire entry, return stake
   - If `entry.stake > remaining_amount`: **split the entry**
     - Return `remaining_amount` to sender
     - Keep `entry.stake - remaining_amount` as a **residual** with the **original timestamp**
     - This preserves the seniority clock for the remaining portion
3. Residuals keep their original `bonded_at_timestamp` — no seniority reset on partial unbonding

#### Example

Validator has entries:
- Entry 0: 200 OPL, bonded at timestamp 1000
- Entry 1: 300 OPL, bonded at timestamp 2000
- Entry 2: 500 OPL, bonded at timestamp 3000

`ValidatorUnbond { amount: 350_OPL }`:
1. Consume Entry 0 entirely (200 OPL returned)
2. Split Entry 1: return 150 OPL, keep remaining 150 OPL with **timestamp 2000** preserved
3. Result: Entry 1 becomes 150 OPL (still bonded at 2000), Entry 2 stays 500 OPL

#### 100 OPL Minimum Enforcement

The minimum only applies to **new bond entries** and **top-ups**, not to residuals left after FIFO splitting. A validator can naturally end up with an entry below 100 OPL after a partial unbond, and that's fine — they can't create new entries below 100 OPL, but existing small entries persist.

---

## 11. Stake-Weighted Mean Age (Planned)

### Current: Per-Entry Weight Sum

```
validator_weight = Σ entry.stake × (1 + ln(1 + entry.age_years))
```

### Planned: Stake-Weighted Mean Age

```
weighted_avg_age = Σ(entry.stake × entry.age_years) / total_stake
validator_weight = total_stake × (1 + ln(1 + weighted_avg_age))
```

This simplifies the weight formula from per-entry accumulation to a single aggregate. Benefits:
- More intuitive — seniority applies to the whole stake, not fragmented
- FIFO unbonding naturally preserves the weighted average
- Easier to reason about for validators and delegators
- Logarithmic diminishing still prevents dominance by early stakers

---

## 12. Fees & Burning

All transaction fees are **permanently burned** — not collected by validators or miners.

- **No minimum fee**: Mempool accepts any transaction, ordered by fee priority
- **No fee schedule**: Market determines inclusion
- **Validator income**: Block rewards only, not fees
- **Deflationary**: Fee burning reduces circulating supply, counterbalancing issuance

Transfer: sender pays `amount + fee`, recipient gets `amount`, `fee` is destroyed.
Bond: sender pays `stake + fee`, `stake` is locked, `fee` is destroyed.
Unbond: entry stake returned to sender, `fee` is burned from sender balance.

Invalid transactions (wrong nonce, insufficient balance, invalid bond_id): no fee burn, no nonce advance.

---

## 13. Block Structure

```rust
BlockHeader {
    height: u64,                        // 0 for genesis
    previous_hash: Hash,                // Blake3-256 of prior block
    state_root: Hash,                   // Blake3-256 of post-execution state
    transaction_root: Hash,              // Blake3-256 commitment of tx IDs
    timestamp: u64,                      // UNIX epoch seconds
    difficulty: u64,                     // Effective difficulty
    pow_proof: Option<Vec<u8>>,         // OMAP/Autolykos nonce (None for PoS)
    validator_signature: Option<Vec<u8>>, // ed25519 (future: BLS) signature
}

Block {
    header: BlockHeader,
    transactions: Vec<Transaction>,
}
```

### Block Hash

`Blake3-256(header_bytes)` where `pow_proof` and `validator_signature` are set to `None` before hashing. The hash is determined before mining — the proof must satisfy the hash, not vice versa.

### Coinbase

The first transaction in each block is a coinbase transaction crediting the block producer with `BASE_REWARD / effective_difficulty × discovery_bonus` plus any burned fees.

---

## 14. Transaction Model

### Types

```rust
enum TransactionAction {
    Transfer { recipient: ObjectId, amount: FlakeAmount },
    ValidatorBond { amount: FlakeAmount },
    ValidatorUnbond { bond_id: u64 },  // Will change to { amount: FlakeAmount }
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
    nonce: u64,                 // Replay protection
    data: Vec<u8>,             // Arbitrary attachment (not consensus-critical)
}
```

### Known Bug: `compute_tx_id`

**CURRENT BUG**: `compute_tx_id` in `crates/wallet/src/signing.rs:120-130` does NOT include the `action` field in the hash. The `_action` parameter is ignored. This means two different transactions (e.g., a transfer and a bond) with the same sender/fee/nonce would produce the same tx_id. **Must be fixed.**

### Transaction Lifecycle

1. **Create**: Wallet signs transaction with ed25519
2. **Submit**: Transaction enters mempool via RPC
3. **Order**: Mempool sorts by fee priority (market-driven, no minimum)
4. **Include**: Miner/validator selects transactions into a block
5. **Execute**: Dispatcher applies state transitions atomically
6. **Persist**: Block and state written to RocksDB atomically

---

## 15. RPC API

JSON-RPC 2.0 server on port 4171.

### Read Endpoints

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
| `opl_getSupply` | _(none)_ | Issued, burned, circulating breakdown |
| `opl_getDifficulty` | _(none)_ | Current difficulty and retarget info |
| `opl_getValidators` | _(none)_ | Active validator set with per-entry bond details |

### Write Endpoints

| Method | Parameters | Description |
|---|---|---|
| `opl_sendTransaction` | `["borsh_hex_string"]` | Submit a Borsh-hex-encoded signed transaction |

### Mining Endpoints

| Method | Parameters | Description |
|---|---|---|
| `opl_getMiningJob` | _(none)_ | Block template for external miners |
| `opl_submitSolution` | `["borsh_hex_string"]` | Submit mined block via mpsc channel |

### Error Codes

JSON-RPC error codes follow Bitcoin's model with Opolys-specific extensions. Errors are structured with `code`, `message`, and optional `data`.

---

## 16. Wallet Key Derivation

### BIP-44 Path

```
m / 44' / 999' / account' / 0'
│    │     │       │        └── change (always 0' for ed25519)
│    │     │       └── account index (0, 1, 2, ...)
│    │     └── SLIP-0044 coin type for Opolys
│    └── BIP-44 purpose (always 44' for BIP-44)
└── master key (hardened)
```

### Mnemonic Format

- 24-word BIP-39 phrase (256 bits of entropy)
- Standard English wordlist with checksum validation
- Optional passphrase (BIP-39 password) for additional security
- Single ed25519 key handles both transaction signing and validator block signing
- Full wallet recovery from mnemonic alone — no separate backup needed

---

## 17. OMAP Proof-of-Work (Planned)

### Why Replace Autolykos

Autolykos is Ergo's algorithm. OMAP is purpose-built for Opolys with better memory-hardness properties and a simpler, more auditable design.

### OMAP Parameters

| Parameter | Value | Description |
|---|---|---|
| `OMAP_DATASET_SIZE` | 512 MiB | Memory requirement per miner |
| `OMAP_NUM_STEPS` | 4,096 | Sequential mixing steps |
| `OMAP_NODE_SIZE` | 1 MiB | Size of each graph node |
| `OMAP_EPOCH_LENGTH` | 1,024 blocks | Dataset refresh interval |
| `OMAP_INNER_HASH` | Blake3-256 | Inner loop hash function |
| `OMAP_FINAL_HASH` | SHA3-256 | Final PoW hash |
| `OMAP_READ_WRITE` | true | Both read AND write memory operations (distinguishes from read-only ASIC designs) |

### OMAP Algorithm (Spec)

```
1. Generate dataset from block header + epoch seed
   - Each node: 1 MiB, 512 nodes total = 512 MiB dataset
   - Blake3(header_bytes || height || node_index) for each node
   
2. Initialize mixer from block header + nonce:
   - mixer = Blake3(previous_hash || state_root || transaction_root || height || timestamp || difficulty || nonce)

3. For each step i in 0..4096:
   a. Derive two dataset indices from mixer bits
   b. READ node1 = dataset[idx1], node2 = dataset[idx2]
   c. MIX: mixer = Blake3(mixer || node1 || node2)
   d. WRITE: dataset[idx1] = Blake3(node1 || mixer)   ← write step (ASIC-resistant!)
   
4. Final hash: pow_hash = SHA3-256(mixer)

5. Valid if: u64(pow_hash[..8]) < (u64::MAX / difficulty)
```

### Key Properties

- **Read+Write**: The write step (3d) means miners must modify the dataset in-place, preventing ASIC-friendly read-only architectures
- **Memory-hard**: 512 MiB minimum, growing with epochs
- **Epoch-based refresh**: Dataset regenerates every 1,024 blocks
- **Dual hash**: Blake3 for inner loop (fast), SHA3-256 for final hash (different security margin)

### Implementation Files (Planned)

- `crates/crypto/src/omap.rs` — Dataset generation, hash computation, mining loop
- `crates/consensus/src/pow.rs` — Replace Autolykos with OMAP calls

---

## 18. BLS12-381 Signatures (Planned)

### Purpose

Aggregate validator signatures for PoS blocks. Instead of including N individual signatures, include one aggregated signature — dramatically reducing block size when there are many validators.

### Spec

- **Curve**: BLS12-381 (pairing-friendly)
- **Aggregation**: Multiple validator signatures on the same block header are aggregated into a single 48-byte signature
- **Verification**: Single pairing check instead of N individual ed25519 verifications
- **Key derivation**: BLS keys derived from the same BIP-39 mnemonic (different derivation path)
- **File**: `crates/crypto/src/bls.rs` (planned)

### Coexistence with ed25519

ed25519 remains for **transaction signing**. BLS is only for **validator block signatures**. This gives us:
- Simple, well-understood transaction signing (ed25519)
- Efficient PoS block validation (BLS aggregation)
- No single point of cryptographic failure

---

## 19. VRF Validator Selection (Planned)

### Purpose

Replace the current weighted random sampling with a Verifiable Random Function for block producer selection. VRF output is unpredictable but verifiable — anyone can confirm the selected validator was legitimate.

### Spec

- **Algorithm**: ECVRF on ed25519 (or BLS12-381) — output is a pseudorandom value plus a proof
- **Input**: Validator private key + current block height + previous block hash
- **Output**: VRF output (for selection) + VRF proof (for verification)
- **Selection**: VRF output is compared to a stake-weighted threshold — if output < threshold(stake, total_stake), the validator is selected
- **File**: `crates/crypto/src/vrf.rs` (planned)

### Benefits Over Current Approach

- **Unpredictable before the fact**: No validator knows if they'll produce the next block until they evaluate their VRF
- **Verifiable after the fact**: Anyone can verify the selection was fair without trusting the validator
- **No seed generation**: No need for on-chain entropy seeds

---

## 20. Stealth Addresses (Planned)

### Purpose

Layer 1 privacy — receivers can generate one-time addresses that are unlinkable to their public identity. Only the sender and receiver can identify the transaction.

### Spec

- **Based on**: ECDH between sender and receiver's viewing key
- **One-time address**: `address = H(sender_ephemeral_pk × receiver_viewing_key || output_index) × G`
- **Detection**: Receiver scans blocks with their viewing key
- **No linkability**: External observers cannot link one-time addresses to the receiver's public identity
- **File**: `crates/crypto/src/stealth.rs` (planned)

---

## 21. Viewing Keys (Planned)

### Purpose

Layer 2 privacy — share a viewing key with an auditor, exchange, or trusted party without giving spending authority. The viewing key reveals transaction history but cannot sign transactions.

### Spec

- **Viewing key**: Derived from the same mnemonic, different path (e.g., `m/44'/999'/1'`)
- **Viewing capability**: See all incoming and outgoing transactions for the associated account
- **Spending prohibition**: Viewing key CANNOT create signatures or authorize transactions
- **Use cases**: Tax reporting, audit compliance, partial transparency

---

## 22. Poseidon Hash (Planced)

### Purpose

ZK-friendly hash function for future SNARK/STARK proofs. Poseidon operates over finite fields, making it significantly more efficient inside zero-knowledge circuits than Blake3 or SHA3.

### Spec

- **Fields**: BLS12-381 scalar field (or appropriate prime field)
- **Rounds**: Full rounds + partial rounds (round numbers TBD based on security analysis)
- **Permutation**: Substitution-Permutation Network with S-boxes
- **Applications**: Future ZK rollups, private transaction proofs, compact proofs of stake
- **File**: `crates/crypto/src/poseidon.rs` (planned)

### Coexistence with Blake3

Blake3 remains the **consensus hash** for block hashes, transaction IDs, and state roots. Poseidon is for **ZK-specific** operations only. The two hash functions serve different purposes and don't conflict.

---

## 23. Networking (libp2p)

### Stack

- **Transport**: libp2p 0.54 with TCP, noise, yamux, and quic
- **Discovery**: Kademlia DHT (bucket size 20)
- **Gossip**: Gossipsub for block and transaction propagation
- **Sync**: Request-response protocol for block/header download
- **Identity**: ed25519-based PeerId

### Status

Scaffold exists in `crates/networking/` with `GossipConfig`, `SyncConfig`, `DiscoveryConfig`. Not yet wired to the node. This is Phase 2 of the build sequence.

---

## 24. Storage

Opolys uses **RocksDB** with **Borsh** serialization:

| Column Family | Key | Value |
|---|---|---|
| `blocks` | `block_<height>` | Borsh-serialized `Block` |
| `accounts` | `account_<hex_object_id>` | Borsh-serialized `Account` |
| `validators` | `validator_<hex_object_id>` | Borsh-serialized `ValidatorInfo` |
| `chain_state` | `chain_state` | Borsh-serialized `PersistedChainState` |

### Reverse Indexes

- `hash_to_height` — Maps Blake3 block hash to block height
- `tx_id_to_location` — Maps transaction ID to `(height, index)` for quick lookups

State is saved atomically after each block is applied. Tests use `tempfile::tempdir()` for isolation.

---

## 25. Architecture & Crate Map

```
Opolys/
├── Cargo.toml                         # Workspace (edition 2024, Rust 1.85+)
├── crates/
│   ├── core/src/
│   │   ├── constants.rs                # All consensus-critical constants
│   │   ├── types.rs                     # Hash, ObjectId, TransactionAction, Block, etc.
│   │   └── errors.rs                    # OpolysError enum
│   ├── crypto/src/
│   │   ├── hash.rs                      # Blake3-256 (Blake3Hasher, hash(), hash_to_object_id)
│   │   ├── signing.rs                   # ed25519 verification
│   │   ├── key.rs                       # KeyPair with ed25519
│   │   └── lib.rs                       # Module declarations
│   │   # PLANNED: bls.rs, poseidon.rs, vrf.rs, omap.rs, stealth.rs
│   ├── consensus/src/
│   │   ├── account.rs                   # AccountStore with fee-burning transfers
│   │   ├── block.rs                     # compute_block_hash(), compute_transaction_root()
│   │   ├── difficulty.rs                # retarget, consensus_floor, discovery_bonus
│   │   ├── emission.rs                  # block reward, validator weight, stake coverage
│   │   ├── mempool.rs                   # Fee-priority mempool with eviction
│   │   ├── pos.rs                      # ValidatorSet with BondEntry, per-entry weight
│   │   ├── pow.rs                      # Autolykos mining (WILL BE REPLACED WITH OMAP)
│   │   └── genesis.rs                   # GenesisAttestation, build_genesis_block
│   ├── storage/src/
│   │   └── store.rs                     # BlockchainStore with reverse indexes
│   ├── execution/src/
│   │   └── dispatcher.rs               # TransactionDispatcher (Transfer, Bond, Unbond)
│   ├── networking/src/                  # Scaffold only (gossip/sync/discovery)
│   ├── wallet/src/
│   │   ├── key.rs                       # KeyPair with ed25519
│   │   ├── bip39.rs                     # BIP-39 mnemonic, SLIP-0010 derivation
│   │   ├── signing.rs                   # TransactionSigner (transfer, bond, unbond)
│   │   ├── account.rs                   # AccountInfo + format_flake_as_opl
│   │   └── lib.rs
│   ├── rpc/src/
│   │   ├── server.rs                    # 16+ RPC endpoints, RpcState, per-entry validator responses
│   │   ├── jsonrpc.rs                   # JsonRpcRequest/Response with specific error codes
│   │   └── lib.rs                       # Exports BlockSubmission, BlockSubmissionResult
│   └── node/src/
│       ├── main.rs                      # Arc<OpolysNode>, mpsc channel for submitSolution, CLI
│       ├── node.rs                      # OpolysNode with Arc<BlockchainStore>, mine/no-rpc config
│       └── lib.rs
```

### Crate Dependency Graph

```
core ← crypto ← consensus ← execution ← node → rpc
                                     ← storage ← node
                                     ← wallet ← node
                                     ← networking ← node
```

### Key External Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `borsh` | 1.5 | Serialization (consensus-critical) |
| `blake3` | 1.8 | Hashing |
| `ed25519-dalek` | 2.1 | Transaction signing |
| `rocksdb` | 0.22 | Persistent storage |
| `libp2p` | 0.54 | P2P networking |
| `tokio` | 1 | Async runtime |
| `axum` | 0.8 | HTTP/RPC server |
| `serde`/`serde_json` | 1.0 | JSON serialization |
| `thiserror` | 2.0 | Error types |
| `anyhow` | 1.0 | Error propagation |
| `bip39` | 2.2 | Mnemonic generation |
| `sha2` | 0.10 | HMAC-SHA512 key derivation |
| `hmac` | 0.12 | HMAC-SHA512 key derivation |

---

## 26. Known Bugs & Fixes

### Bugs Fixed (Earlier Sessions)

| Bug | Fix |
|---|---|
| ValidatorBond had no amount field | Added `amount: FlakeAmount` to `ValidatorBond` |
| Block hash chain was always zero | Implemented `compute_block_hash()` that hashes all header fields |
| BIP39 was broken | Switched to `bip39` crate with SLIP-0010 ed25519 derivation |
| `Instant` not found in jsonrpc.rs | Added `use std::time::Instant` |
| Unused imports across 7 crates | Cleaned up all warnings |
| Deprecated `word_iter` in bip39.rs | Changed to `words()` |
| Duplicate imports in main.rs | Fixed |
| Test isolation: Node tests sharing `./data` RocksDB | Switched to `tempfile::tempdir()` |
| Doctest scoping issues | Fixed with proper `use` prefixes |

### Bugs Fixed (This Session)

| Bug | Fix |
|---|---|
| Dilithium/quantum signatures removed | Deleted `dilithium.rs`, `hybrid_keypair.rs`, rewrote `bip39.rs` for ed25519-only. Net: 427 lines deleted, 45 inserted |
| Circular dependency node ↔ rpc | RPC depends only on core/consensus/storage/execution, not node |
| `BlockchainStore` not Clone | Wrapped in `Arc<BlockchainStore>` |
| Node moved into tokio::spawn twice | Wrapped in `Arc<OpolysNode>`, cloned for each task |
| Bond nonce not incrementing | Added `nonce += 1` after successful bond in dispatcher |

### Bug Still Present (Needs Fix)

| Bug | File | Description |
|---|---|---|
| `compute_tx_id` ignores action | `crates/wallet/src/signing.rs:120-130` | The `_action` parameter is not included in the hash. Two different transactions with same sender/fee/nonce produce identical tx_ids. Must include Borsh-serialized action in the hash input. |

---

## 27. Build Sequence

The ordered list of everything that needs to happen, from current state to mainnet:

### Phase 1: Current Fixes (In Progress)

| Step | Status | Description |
|---|---|---|
| 1A | **NEEDS FIX** | Fix `compute_tx_id` — include `action` in the hash |
| 1B | **PLANNED** | FIFO unbonding: `ValidatorUnbond { amount }` replaces `{ bond_id }` |
| 1C | **PLANNED** | Stake-weighted mean age: `weight = total_stake × (1 + ln(1 + weighted_avg_age))` |

### Phase 2: Cryptographic Upgrades

| Step | Status | Description |
|---|---|---|
| 2A | **PLANNED** | OMAP PoW — replace Autolykos with 512 MiB read+write memory-hard mining |
| 2B | **PLANNED** | BLS12-381 signature aggregation for PoS blocks |
| 2C | **PLANNED** | VRF for unpredictable, verifiable validator selection |

### Phase 3: Privacy

| Step | Status | Description |
|---|---|---|
| 3A | **PLANNED** | Stealth addresses (Layer 1 receiver privacy) |
| 3B | **PLANNED** | Viewing keys (Layer 2 selective transparency) |

### Phase 4: Networking

| Step | Status | Description |
|---|---|---|
| 4A | **PLANNED** | Wire libp2p gossip for block/transaction propagation |
| 4B | **PLANNED** | Chain sync protocol (header-first, then block bodies) |
| 4C | **PLANNED** | Peer discovery via Kademlia DHT |

### Phase 5: ZK Foundation

| Step | Status | Description |
|---|---|---|
| 5A | **PLANNED** | Poseidon hash integration (ZK-friendly hash for future proofs) |

### Phase 6: Staking & PoS Block Production

| Step | Status | Description |
|---|---|---|
| 6A | **PLANNED** | `--validate` flag for validator mode |
| 6B | **PLANNED** | PoS block production based on VRF output |
| 6C | **PLANNED** | BLS aggregated attestation |
| 6D | **PLANNED** | PoW/PoS phase transition logic |

### Phase 7: Security Hardening

| Step | Status | Description |
|---|---|---|
| 7A | **PLANNED** | Code audit — overflow checks, edge cases, consensus logic |
| 7B | **PLANNED** | Fuzz testing — all parsers, deserializers, edge cases |
| 7C | **PLANNED** | User-facing attack surface review |

### Phase 8: Genesis & Launch

| Step | Status | Description |
|---|---|---|
| 8A | **PLANNED** | Genesis ceremony — lock in real market data |
| 8B | **PLANNED** | Testnet deployment |
| 8C | **PLANNED** | Mainnet launch |

---

## 28. Test Count

**125 tests passing** across 9 crates (as of last run).

Distribution:
- `opolys-core`: 12 tests
- `opolys-crypto`: 8 tests
- `opolys-consensus`: 18 tests
- `opolys-storage`: 5 tests
- `opolys-execution`: 11 tests
- `opolys-wallet`: 18 tests
- `opolys-rpc`: 6 tests
- `opolys-node`: 2 tests (ignored)
- Other/integration: ~51 tests

---

## Appendix: Key Formulas Reference

### Block Reward
```
block_reward = BASE_REWARD / effective_difficulty × discovery_bonus
BASE_REWARD = 440,000,000 flakes (440 OPL)
```

### Effective Difficulty
```
effective_difficulty = max(retarget, consensus_floor, MIN_DIFFICULTY)
```

### Difficulty Retarget
```
new_difficulty = old_difficulty × (actual_time / expected_time)
clamped to [old/4, old×4], floored at MIN_DIFFICULTY
```

### Consensus Floor
```
consensus_floor = total_issued / bonded_stake
```

### Discovery Bonus
```
ratio = u64::MAX / (difficulty × hash_value)
bonus = max(1, floor(√ratio))
```

### Validator Weight (Current)
```
weight = Σ entry.stake × (1 + ln(1 + entry.age_years))
```

### Validator Weight (Planned)
```
weighted_avg_age = Σ(entry.stake × entry.age_years) / total_stake
weight = total_stake × (1 + ln(1 + weighted_avg_age))
```

### Stake Coverage
```
stake_coverage = min(1.0, total_bonded / total_issued)
pow_share = 1.0 - stake_coverage
pos_share = stake_coverage
```

### OMAP PoW Verification (Planned)
```
valid if: u64(pow_hash[..8]) < (u64::MAX / difficulty)
pow_hash = SHA3-256(final_mixer)
```

### Gold Derivation
```
annual_gold_production = 3,630 tonnes
annual_oz = 3,630 × 32,150.7 ≈ 116,707,041 troy oz
blocks_per_year = 365.25 × 86400 / 120 ≈ 262,980
BASE_REWARD = floor(116,707,041 / 262,980) = 440 OPL
```

---

*This document is the single source of truth for Opolys development. Update it with every design decision and implementation change.*