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
23. [Key Formulas Reference](#key-formulas-reference)
24. [Security Audit & Bug Tracker](#24-security-audit--bug-tracker)
25. [Implementation Plan — Pass 1](#25-implementation-plan--pass-1)

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
| `ANNUAL_ATTRITION_PERMILLE` | `15` (1.5%) | Annual gold attrition rate in permille |
| `BLOCKS_PER_YEAR` | `350,640` | Approximate blocks per year (365.25 × 86400 / 90) |
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
| `DEFAULT_LISTEN_PORT` | `4,170` | P2P listen port |
| `MAINNET_CHAIN_ID` | `1` | Cross-chain replay protection |
| `MAX_TRANSACTIONS_PER_BLOCK` | `10,000` | Max transactions per block |
| `MAX_BLOCK_SIZE_BYTES` | `10,485,760` (10 MiB) | Max serialized block size |
| `MAX_TX_DATA_SIZE_BYTES` | `1,024` (1 KiB) | Max transaction data field |
| `MAX_FUTURE_BLOCK_TIME_SECS` | `300` (5 min) | Max future block time skew |
| `MAX_SLASH_EVIDENCE_PER_BLOCK` | `10` | DoS cap on ed25519 verifications per block |
| `GENESIS_DIFFICULTY` | `7` | Initial difficulty at launch |

### Mempool Constants

| Constant | Value | Description |
|---|---|---|
| `MEMPOOL_MAX_SIZE_BYTES` | `100,000,000` (100 MiB) | Max mempool size in bytes |
| `MEMPOOL_MAX_TXS_PER_ACCOUNT` | `50` | Max pending transactions per sender |
| `MEMPOOL_TX_EXPIRY_SECS` | `86,400` (24 hours) | Transaction expiry time |
| `TX_MAX_SIZE_BYTES` | `100,000` (~100 KiB) | Max single transaction size |
| `MAX_NONCE_GAP` | `10` | Reject transactions more than 10 nonces ahead |
| `CAPACITY_RATIO` | `10` | Mempool capacity in blocks (= MAX_BLOCK_SIZE_BYTES / MEMPOOL_MAX_SIZE_BYTES, rounded) |
| `CONGESTION_THRESHOLD_PERMILLE` | `100` (10%) | Mempool usage threshold for rush mode (1000 / CAPACITY_RATIO) |

### Mempool Congestion Pricing

Two-state model derived from capacity ratio (`CAPACITY_RATIO = MEMPOOL_MAX_SIZE_BYTES / MAX_BLOCK_SIZE_BYTES ≈ 10`):

| State | Condition | Effective Min Fee |
|---|---|---|
| **Spot** | Usage ≤ 10% (1 block's worth) | `max(MIN_FEE, suggested_fee)` |
| **Rush** | Usage > 10% | `max(MIN_FEE, suggested_fee × CAPACITY_RATIO)` |

The congestion threshold is `CONGESTION_THRESHOLD_PERMILLE = 1000 / CAPACITY_RATIO = 100` (100 permille = 10%). When the mempool holds more than one block's worth of transactions, fees scale by the capacity ratio (~10×), reflecting that transactions must outcompete ~10 blocks worth of pending data to be included next.

### Mempool Same-Nonce Replacement

A transaction replacing another with the same nonce must pay at least 10% more: `min_replacement_fee = old_priority × 11 / 10`.

### Networking Constants

| Constant | Value | Description |
|---|---|---|
| `MAX_INBOUND_CONNECTIONS` | `50` | Max inbound P2P connections |
| `MAX_OUTBOUND_CONNECTIONS` | `50` | Max outbound P2P connections |
| `MAX_PEER_COUNT` | `200` | Max total peers |
| `SYNC_MAX_BLOCKS_PER_REQUEST` | `500` | Max blocks per sync response |
| `SYNC_MAX_HEADERS_PER_REQUEST` | `2,000` | Max headers per sync response |
| `SYNC_REQUEST_TIMEOUT_SECS` | `30` | Sync request timeout |
| `SYNC_PARALLEL_PEER_COUNT` | `3` | Parallel sync peers |
| `KAD_BUCKET_SIZE` | `20` | Kademlia DHT bucket size |
| `PING_INTERVAL_SECS` | `30` | Ping interval |
| `PING_TIMEOUT_SECS` | `20` | Ping timeout |
| `GOSSIP_MAX_MESSAGE_SIZE_BYTES` | `10,485,760` (10 MiB) | Max gossip message size (= MAX_BLOCK_SIZE_BYTES) |

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

1. `RefinerBond { amount }` — Lock `amount` OPL as stake, plus a bond assay of `amount × 0.375%` (permanently burned). Creates a new entry if the refiner already exists (top-up).
2. `RefinerUnbond { amount }` — Withdraw `amount` OPL using **FIFO order** (see Section 9), plus an unbond assay of `unbonded × 0.375%` (permanently burned). The sender must have sufficient balance to pay both the transaction fee and the unbond assay.
3. **Slashing** — Only for double-signing. **100% of stake burned** on any double-sign offense. No graduated penalties, no offense counter, no reset window. Slashed stake is removed from circulation, not confiscated to any treasury.

### Stake Decay

Bonded stake decays once per epoch (960 blocks) at an annual rate of `ANNUAL_ATTRITION_PERMILLE` (1.5%):

```
decay_numerator = 1_000_000 - (ANNUAL_ATTRITION_PERMILLE × 1000 / 365)
               ≈ 999_959
entry.stake = entry.stake × decay_numerator / 1_000_000
```

All decayed stake is permanently burned. This mirrors vault storage fees in gold custody.

### Per-Entry Weight

Each `BondEntry` has its own seniority clock:

```
entry_weight = entry.stake × (1 + ln(1 + entry.age_years))
```

Logarithmic seniority means older entries earn more per-coin, but the marginal gain diminishes — preventing permanent dominance by early stakers.

### Refiner vs Miner Block Reward

```
refiner_block_reward = BASE_REWARD / difficulty × 1.0 (flat, no vein yield)
miner_block_reward    = BASE_REWARD / difficulty × vein_yield (1.0x to ~10x)
```

Refiners earn steady predictable income (like gold vaults), miners earn variable income based on luck (like gold miners). Refiner blocks pass `hash_int = 0` to `compute_vein_yield()`, which returns the 1.0x floor by design.

Genesis block (height 0) has zero reward: `block_reward = 0`.

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

### Supply Attrition

Opolys mirrors physical gold attrition (~1.5% of above-ground gold lost annually, USGS/WGC). Three channels burn OPL from circulation:

| Channel | Rate | Formula | Gold Analogy |
|---|---|---|---|
| **Mine assay** | ~1.5%/year of issuance | `block_reward × ANNUAL_ATTRITION_PERMILLE / 1000` | Processing waste |
| **Stake decay** | ~1.5%/year of bonded stake | `entry.stake × (ANNUAL_ATTRITION_PERMILLE × 1000 / 365) / 1_000_000` per epoch | Vault storage fees |
| **Bond assay** | 0.375% of bonded amount | `amount × ANNUAL_ATTRITION_PERMILLE / 4 / 1000` | Assay fee to enter vault |
| **Unbond assay** | 0.375% of unbonded amount | `amount × ANNUAL_ATTRITION_PERMILLE / 4 / 1000` | Assay fee to exit vault |

- Bond assay and unbond assay are each `ANNUAL_ATTRITION_PERMILLE / 4 / 1000 = 0.00375 = 0.375%`
- Combined (bond + unbond), this is `0.75%` per round trip
- Stake decay is applied once per epoch (960 blocks) at a per-epoch rate derived from the annual rate
- All attrition is permanently burned — no recipient, no treasury

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

Vein yield uses a `sqrt(ln)` formula that makes rich veins genuinely rare — matching real gold's log-normal grade distribution:

```
yield_milli = 1000 + sqrt(ln(target / hash_int)) × 1000
vein_yield = yield_milli / 1000.0
```

Where:
- `target = 2^(64-D) - 1` for difficulty D (leading zero bits model)
- `hash_int` = first 8 bytes of the EVO-OMAP PoW hash, interpreted as big-endian u64
- `ln()` returns 0 when `hash_int <= 0` or `hash_int >= target`, giving the 1.0× floor

The `sqrt(ln)` formula produces a natural distribution with rare bonanzas:

| Yield | Frequency |
|---|---|
| 1.0× (floor) | hash fails to beat target |
| ~1.89× | mean yield for valid PoW |
| 2.0× | ~1 in 2 blocks |
| 3.0× | ~1 in 50 blocks (~75 min) |
| 5.0× | Essentially never (~1 in 10^9) |
| 10.0× | Impossible (out of range) |

Implementation uses `f64::ln()` and `f64::sqrt()`. IEEE 754 guarantees deterministic results across all platforms.

### Vein Bonus Isolation (Coverage Split)

The coverage-based reward split applies to **base_reward only**. The vein bonus goes 100% to the miner:

```
miner_share = base_share + vein_bonus
refiner_share = coverage_milli × base_reward × vein_multiplier / 1000
```

Where `vein_multiplier = 1.0` for refiners (no vein yield) and `vein_yield` for miners.

Gold analogy: refineries charge per ounce processed, not per ore grade. The miner keeps the ore premium.

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

### Transaction Root

`compute_transaction_root` computes a Merkle-like root over all transactions. Each transaction contributes `Blake3(tx_id || fee || nonce)` — the fee and nonce are committed to prevent tampering with fee replacements.

### State Root

`Blake3(account_root || refiner_root || total_issued || total_burned || height)` — the state root commits to the full post-execution state: all accounts, the refiner set, total issued OPL, total burned OPL, and the block height.

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
- **Ping**: Liveness checks with 30s interval, 20s timeout
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
| 10: Staking | **DONE** | `--refine`, 100% slash on double-sign, timeout-based refiner block production |
| 11: Security | **DONE** | Eclipse protection, subnet diversity, DoS limits, memory challenge |
| 12: Pass 1 | **IN PROGRESS** | Phase A DONE (ec0df9b), M19 DONE (2cf09c2), H3+H4 FIXED, L7+L10 FIXED, economic model (vein yield, assay, decay, two-state fees) DONE (07da54b). Phase B–E remaining security & protocol fixes |
| 13: Pass 2 | **PLANNED** | Attestations, reliability score, block confidence |
| 14: Mainnet | **READY** | Genesis ceremony and launch (after Pass 1+2) |

---

## 21. Test Count

**171 tests passing** across 10 crates (as of commit 07da54b):
- `opolys-consensus`: 95 tests
- `opolys-core`: 13 tests
- `opolys-crypto`: 8 tests
- `opolys-storage`: 6 tests

Run with `cargo test --workspace`.

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
8. **Transaction root**: Must match `compute_transaction_root(block.transactions)`. Computed as `Blake3(tx_id || fee || nonce)` per transaction, then a Merkle-like root over all entries.
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

### Mempool Inclusion Rules

1. **Fee minimum**: Must meet `MIN_FEE` (1 Flake), or `2× MIN_FEE` if mempool >80% full, or `10× MIN_FEE` if >95% full
2. **Nonce gap**: Must be within `MAX_NONCE_GAP` (10) of the account's current nonce
3. **Same-nonce replacement**: Must pay at least 10% more than the existing transaction's priority
4. **Per-account limit**: Max `MEMPOOL_MAX_TXS_PER_ACCOUNT` (50) pending transactions per sender
5. **Expiry**: Transactions expire after `MEMPOOL_TX_EXPIRY_SECS` (86,400 = 24 hours)

---

## 23. Key Formulas Reference

### Block Reward
```
yield_milli = 1000 + sqrt(ln(target / hash_int)) × 1000   // integer, rounded to nearest milli
vein_yield = yield_milli / 1000.0
mine_assay = block_reward × ANNUAL_ATTRITION_PERMILLE / 1000  // burned at source
block_reward = (BASE_REWARD / difficulty) × vein_yield
miner_share = base_share + vein_bonus                   // vein bonus goes 100% to miner
```

### Effective Difficulty
```
effective_difficulty = max(retarget, consensus_floor, MIN_DIFFICULTY)
```

### Difficulty Retarget
```
new_difficulty = old_difficulty × expected_time / actual_time
```
No maximum clamp. Floor is MIN_DIFFICULTY (1). Uses truncating integer division (see M18 for bias). If `actual_time == 0` (timestamp collision), difficulty spikes to `old_difficulty × 4` as a safety measure.

### Consensus Floor
```
consensus_floor = total_issued / bonded_stake
```
Returns 0 when `bonded_stake = 0` (no refiners yet).

### Genesis Block Reward
```
block_reward = 0  // for height == 0
```
The genesis block produces no OPL. First OPL enters circulation at block 1.

### State Root
```
state_root = Blake3(account_root || refiner_root || total_issued || total_burned || height)
```
Where `account_root` is the Merkle-like root of all account states, and `refiner_root` is the root of the serialized refiner set.

### Suggested Fee
```
suggested_fee = EMA(total_fees_burned, previous_suggested_fee)
              = (burned + 9 × old) / 10, floored at MIN_FEE
```
Computed from `total_fees_burned` (actually burned by successful transactions), not `total_fees` (declared). This prevents failed transactions from inflating the fee signal.

### Mempool Congestion Pricing

Two-state model derived from capacity ratio:

| State | Condition | Effective Min Fee |
|---|---|---|
| **Spot** | Usage ≤ 10% (1 block's worth) | `max(MIN_FEE, suggested_fee)` |
| **Rush** | Usage > 10% | `max(MIN_FEE, suggested_fee × CAPACITY_RATIO)` |

Where `CAPACITY_RATIO = MEMPOOL_MAX_SIZE_BYTES / MAX_BLOCK_SIZE_BYTES ≈ 10` and `CONGESTION_THRESHOLD_PERMILLE = 1000 / CAPACITY_RATIO = 100` (100 permille = 10%).

### Supply Attrition

Three channels permanently burn OPL, mirroring physical gold attrition (~1.5%/year):

| Channel | Rate | Formula |
|---|---|---|
| Mine assay | ~1.5%/yr of issuance | `block_reward × ANNUAL_ATTRITION_PERMILLE / 1000` |
| Stake decay | ~1.5%/yr of bonded stake | `entry.stake × decay_numerator / 1_000_000` per epoch |
| Bond assay | 0.375% of bonded amount | `amount × ANNUAL_ATTRITION_PERMILLE / 4 / 1000` |
| Unbond assay | 0.375% of unbonded amount | `amount × ANNUAL_ATTRITION_PERMILLE / 4 / 1000` |

Where `decay_numerator = 1_000_000 - (ANNUAL_ATTRITION_PERMILLE × 1000 / 365) ≈ 999_959`.

### Same-Nonce Replacement

A transaction replacing another with the same sender and nonce must pay at least 10% more:
```
min_replacement_fee = old_priority × 11 / 10
```

### Nonce Gap Limit

Transactions with `nonce > account.nonce + MAX_NONCE_GAP` (10) are rejected to prevent slot squatting.

### Ban Escalation

Misbehaving peers are banned with escalating durations:

| Offense Count | Ban Duration |
|---|---|
| 1st | 1 hour |
| 2nd | 24 hours |
| 3rd | 7 days |
| 4th+ | Permanent |

### Refiner Weight
```
entry_weight = entry.stake × (1 + ln(1 + entry.age_years))
```

### Stake Coverage
```
coverage_milli = (bonded_stake × 1000) / total_issued   // integer, no float
miner_share_amount = block_reward × (1000 - coverage_milli) / 1000
refiner_share_amount = block_reward - miner_share_amount
```
Miner share goes to the block producer. Refiner share is distributed among active refiners proportional to weight.

### EVO-OMAP PoW Verification
```
target = 2^(64-D) - 1    // D = difficulty (leading zero bits)
valid if: u64(pow_hash[..8]) <= target
pow_hash = SHA3-256(state_summary || commitment_hash || memory_commitment)
```
EVO-OMAP difficulty means **leading zero bits** in the SHA3-256 output, NOT a u64 divisor.

---

## 24. Security Audit & Bug Tracker

Every bug below includes: **What it is**, **Why it matters**, and **How to fix it**.

### CRITICAL (4)

#### C1: Testnet/mainnet shared data directory
**Location:** `node.rs:395-458`
**Status:** OPEN

**What it is:** The node uses a single `data_dir` (default `./data`) regardless of chain. If a user runs with `--testnet` (which loads testnet genesis), then restarts without `--testnet`, the mainnet node loads the testnet balances and refiner set from the same RocksDB database. The genesis accounts, chain state, and refiners from the testnet run are accepted as valid mainnet state.

**Why it matters:** An attacker could inflate balances on testnet, then switch to mainnet mode and spend those inflated balances on mainnet. This is a chain-splitting vulnerability — different nodes with different histories would disagree on account balances.

**How to fix:** Add a `chain_id` field to `PersistedChainState`. On load, compare it against `MAINNET_CHAIN_ID`. If they don't match, refuse to load and start fresh with a warning. Also partition the data directory: use `data_dir/mainnet/` vs `data_dir/testnet/` subdirectories.

---

#### C2: Mnemonic passed as CLI argument
**Location:** `wallet/main.rs:41-118`
**Status:** OPEN

**What it is:** The wallet CLI accepts the 24-word mnemonic as a positional argument (e.g., `opl transfer "word1 word2 ... word24" ...`). This means the seed phrase is visible in `ps aux`, shell history (`~/.bash_history`, `~/.zsh_history`), and `/proc/<pid>/cmdline` on Linux. Anyone with shell access can recover the private key.

**Why it matters:** The mnemonic is the master seed — possessing it gives full control of all derived accounts. Shell history is often backed up, shared, or logged. Process arguments are visible to all users on multi-user systems.

**How to fix:** Replace positional mnemonic arguments with `OPOLYS_MNEMONIC` environment variable or interactive stdin prompt via `rpassword::read_password()`. The `Address`, `Transfer`, `Bond`, `Unbond` subcommands should accept `--from-env` (reads `OPOLYS_MNEMONIC`) or `--from-stdin` (reads interactively). Mnemonic is never a CLI arg.

---

#### C3: No memory zeroing for private keys
**Location:** `wallet/bip39.rs:94` (`DerivedSeed`), `wallet/key.rs:60` (`KeyPair`)
**Status:** OPEN

**What it is:** `DerivedSeed` and `KeyPair` both derive `Debug`, meaning their fields can be printed to logs. Neither type implements `Zeroize` — when they go out of scope, private key material remains in memory until the allocator reuses those pages. Stack temporaries (like `seed` at `key.rs:75`) are also not zeroed.

**Why it matters:** Private key material persists in RAM after use. A core dump, debugger attach, or memory scan can extract keys. The `#[derive(Debug)]` on both types means `{:?}` formatting will print the raw seed bytes to any log stream.

**How to fix:** (1) Remove `#[derive(Debug)]` from both `DerivedSeed` and `KeyPair`. Implement manual `Debug` impls that print `"DerivedSeed([REDACTED])"` and `"KeyPair { signing_key: [REDACTED], ... }"`. (2) Add `impl Zeroize` for both types using the `zeroize` crate. In `Drop`, call `self.zeroize()`. (3) Zero the `seed` array in `KeyPair::generate()` after constructing the `SigningKey`.

---

#### C4: RPC write/mining endpoints unauthenticated by default
**Location:** `rpc/server.rs:169-190, 221-222`
**Status:** OPEN

**What it is:** The RPC server has an optional `api_key` configuration, but it defaults to `None`. When `api_key` is `None`, write endpoints (`opl_sendTransaction`, `opl_getMiningJob`, `opl_submitSolution`) are accessible without any authentication. Additionally, at lines 180 and 185, the key comparison uses `==` on strings, which short-circuits on the first mismatching byte, enabling timing attacks.

**Why it matters:** Anyone on the network can submit arbitrary transactions, waste mining jobs, or submit invalid blocks. The timing attack allows recovering the API key byte-by-byte in O(256 × key_length) requests.

**How to fix:** (1) Default to a randomly-generated API key printed at startup. Only allow `None` (no auth) via explicit `--no-rpc-auth` flag. (2) Replace `==` with constant-time comparison via `subtle::ConstantTimeEq`.

---

### HIGH (9 — H3 and H4 fixed)

#### H1: No hash domain separation
**Location:** `crypto/hash.rs`, `wallet/signing.rs:145`, `node.rs:754`, multiple
**Status:** OPEN

**What it is:** All Blake3-256 hashes are computed over raw domain data with no prefix tag. The same hash function is used for: transaction IDs, block hashes, ObjectIds, state roots, Merkle roots, and refiner signatures. A hash collision in one domain could be confused with a hash in another.

**Why it matters:** Without domain separation, a chosen-prefix collision attack on Blake3 (if ever found) would apply across all domains simultaneously. A hash output from one context could be misinterpreted in another.

**How to fix:** Add domain tags to every `Blake3Hasher::update()` call. Define constants: `DOMAIN_TX_ID`, `DOMAIN_BLOCK_HASH`, `DOMAIN_OBJECT_ID`, `DOMAIN_STATE_ROOT`, `DOMAIN_TX_ROOT`, `DOMAIN_REFINER_SIG`. Prepend `hasher.update(domain_tag)` before `hasher.update(data)` in every hashing call.

---

#### H2: No signing domain separation
**Location:** `wallet/signing.rs:46`, `node.rs:754`
**Status:** OPEN

**What it is:** The same ed25519 key signs both transactions and refiner blocks using the same serialization format. At `signing.rs:46`, the signed data is `borsh(sender, action, fee, nonce, chain_id)`. At `node.rs:754`, the refiner signature signs the raw block hash bytes. A specially crafted transaction could be a valid signature of a block hash, or vice versa.

**How to fix:** Prepend a domain tag to all signed data: Transactions sign `b"OPL_TX_V1" || borsh(...)`; Refiner blocks sign `b"OPL_REF_BLOCK_V1" || block_hash_bytes`.

---

#### ~~H3: Suggested fee computed from declared fees, not burned fees~~ — **FIXD** (07da54b)
**Location:** `node.rs:1001-1002`
**Status:** **FIXD**

**What it was:** `total_fees` summed `tx.fee` from all transactions, including failed ones whose fees were not actually burned. This overstated the fee market signal.

**How fixed:** `compute_suggested_fee()` now uses `total_fees_burned` (actually burned by successful transactions) instead of `total_fees`.

---

#### ~~H4: Unbond fee bypass — zero-balance accounts unbond for free~~ — **FIXD** (07da54b)
**Location:** `dispatcher.rs:241-304`
**Status:** **FIXD**

**What it was:** When processing `RefinerUnbond`, the fee burn was conditional: `if account.balance >= fee`. If balance was insufficient, the fee was silently skipped but `ApplyResult::ok(fee)` still reported the fee as burned. Accounts with zero balance could unbond without paying fees.

**How fixed:** (1) Introduced unbond assay (0.375% of unbonded amount) that must be paid from the sender's balance. (2) Pre-check total fees (tx.fee + unbond_assay) before executing the unbond. If insufficient balance, the transaction is rejected entirely with no state change.

---

#### H5: RocksDB non-atomic cross-CF writes
**Location:** `store.rs:103,110, 280,287`
**Status:** OPEN

**What it is:** Each column family is saved independently. `save_block()` writes the block, then `save_block_indexes()` writes indexes separately. A crash between them leaves the database inconsistent.

**How to fix:** Use RocksDB `WriteBatch` to batch all writes within a single atomic commit per block application.

---

#### H6: No size limit on sync response bodies
**Location:** `network.rs:204-207`
**Status:** OPEN

**What it is:** `request_response::Config::default()` places no limit on response body size. A malicious peer can respond with a multi-gigabyte CBOR payload, causing OOM.

**How to fix:** Set `with_max_response_size(10 * 1024 * 1024)` and `with_request_timeout(Duration::from_secs(sync_config.request_timeout_secs))`.

---

#### H7: request_timeout_secs defined but never applied
**Location:** `network.rs:204-207`, `sync.rs:49`
**Status:** OPEN

**What it is:** `SyncConfig.request_timeout_secs: 30` is defined but never passed to the `request_response::Config`. The config uses `Config::default()` which ignores it.

**How to fix:** Wire the timeout: `.with_request_timeout(Duration::from_secs(config.sync_config.request_timeout_secs))`.

---

#### H8: Block indexes saved separately from block data
**Location:** `store.rs:155 vs 95`
**Status:** OPEN

**What it is:** `save_block()` and `save_block_indexes()` are called separately. A crash between them leaves the block unreachable by hash.

**How to fix:** Merge all block-related writes into a single `WriteBatch` (also addresses H5).

---

#### H9: No integrity checksums on persisted data
**Location:** `store.rs` throughout
**Status:** OPEN

**What it is:** RocksDB values are stored without integrity checks. Bit rot or disk corruption causes silent data loss.

**How to fix:** Prepend a 16-byte BLAKE3 checksum to every stored value. Verify on load. Wrap all `save_*` and `load_*` methods.

---

### MEDIUM (23 — 2 fixed, 2 by design)

#### ~~M1: Graduated slashing~~ — **FIXD** (ec0df9b)
**Location:** ~~`pos.rs:500-553`~~ → `refiner.rs:488`
Replaced 10%/33%/100% graduated slash with 100% burn on any double-sign. No offense counter, no reset window.

#### ~~M2: Slash reset window~~ — **FIXED** (ec0df9b)
`slash_offense_count` and `last_slash_height` fields deleted from `RefinerInfo`. The 10,240-block reset window has been removed entirely.

#### M3: PoS signature verification runs on PoW blocks
**Location:** `node.rs:825-854`
**Status:** OPEN
**What it is:** At line 825, `if block.header.refiner_signature.is_some()` runs the ed25519 verification path for any block that has a `refiner_signature` field, even if it also has `pow_proof`. A PoW block with a fake `refiner_signature` would enter the sig verification path and could be rejected even if the PoW proof is valid.
**How to fix:** Check mutual exclusivity before verification. Only verify refiner signature if `pow_proof.is_none() && refiner_signature.is_some()`.

#### M4: PoW/PoS mutual exclusivity not checked
**Location:** `block.rs:270-308`
**Status:** OPEN
**What it is:** `validate_block()` doesn't reject blocks that have both `pow_proof` and `refiner_signature`. A block with both would be accepted.
**How to fix:** Add explicit check: reject if both are `Some()`.

#### M5: Producer field not validated for PoW blocks
**Location:** `node.rs:971`
**Status:** OPEN
**What it is:** Any ObjectId can be set as producer and will receive block rewards, even without a registered account.
**How to fix:** Require that the producer has a registered account with a public key.

#### M6: Zero-miner-id phantom issuance
**Location:** `node.rs:971` vs `node.rs:1005`
**Status:** OPEN
**What it is:** If `block.header.producer` is the zero ObjectId, PoW share is skipped but `total_issued` still increments. OPL is "issued" to nobody — phantom inflation.
**How to fix:** Reject blocks with zero producer at height > 0 in `validate_block()`.

#### M7: API key timing attack
**Location:** `rpc/server.rs:178-190`
**Status:** OPEN
**What it is:** `check_api_key()` uses `==` on strings, which short-circuits on first mismatching byte. Enables byte-by-byte key recovery via timing.
**How to fix:** Use `subtle::ConstantTimeEq`: `bool::from(provided.as_bytes().ct_eq(required.as_bytes()))`.

#### M8: CORS allows all origins
**Location:** `rpc/server.rs:926-927`
**Status:** OPEN
**What it is:** `CorsLayer::new().allow_origin(Any)` allows any website to make RPC requests.
**How to fix:** Restrict to `["http://localhost:4171", "http://127.0.0.1:4171"]`.

#### M9: No request body size limit
**Location:** `rpc/server.rs:486-499`
**Status:** OPEN
**What it is:** No limit on JSON-RPC request body size. Multi-gigabyte hex strings cause OOM.
**How to fix:** Use Axum's `DefaultBodyLimit::max(1_048_576)` (1 MiB).

#### M10: Key file world-readable
**Location:** `wallet/key.rs:190-193`
**Status:** OPEN
**What it is:** `fs::write(&key_path, &key_bytes)` writes the raw 32-byte ed25519 seed with default permissions (often 644).
**How to fix:** Set permissions to 0600 after writing. (Pass 2: encrypt with passphrase.)

#### ~~M11: Mempool expiry not enforced~~ — **FIXED**
#### ~~M12: Mempool doesn't check minimum fee~~ — **FIXED**

#### M13: Ephemeral P2P keypair
**Location:** `network.rs:167`
**Status:** OPEN
**What it is:** `libp2p::identity::Keypair::generate_ed25519()` creates a new keypair on every restart, giving a new PeerId. Breaks DHT routing, bypasses bans, wastes bandwidth.
**How to fix:** Persist keypair to `$DATA_DIR/network_keypair.ed25519`. Load on restart.

#### M14: TOCTOU race — height check vs apply_block
**Location:** `main.rs:760` (gossip), `main.rs:395` (mining)
**Status:** OPEN
**What it is:** Height is read under a read lock, then `apply_block()` acquires a write lock later. Between the two, another thread could apply a block at the same height.
**How to fix:** Acquire write lock first, do height check inside the write lock.

#### M15: Sync start_height unvalidated
**Location:** `main.rs:963-993`
**Status:** OPEN
**What it is:** Sync handler doesn't validate `start_height` is within `[0, current_height]`. Malicious peers can request the entire chain repeatedly.
**How to fix:** Clamp `start_height` to `[0, current_height]` and limit to `max_blocks_per_request` blocks.

#### M16: No WAL sync mode on RocksDB
**Location:** `store.rs:74`
**Status:** OPEN
**What it is:** Default RocksDB options don't fsync the WAL after writes. Data may be lost on power failure.
**How to fix:** Use `WriteOptions::set_sync(true)` on critical writes (block saves, state saves).

#### ~~M17: difficulty_to_target(1) = 2^63-1~~ — **BY DESIGN**
Difficulty 1 means "1 leading zero bit", so ~50% of random hashes pass. Correct for the leading-zero-bits interpretation.

#### M18: Integer division bias in retarget
**Location:** `difficulty.rs:133`
**Status:** OPEN
**What it is:** `old_difficulty * expected_time / actual_time` truncates toward zero, creating systematic downward bias over many epochs.
**How to fix:** Use rounding division: `((numerator + actual/2) / actual)`.

#### ~~M19: Dead code: compute_pow_share/compute_pos_share use f64~~ — **FIXED** (2cf09c2)
**Location:** ~~`emission.rs:174-193`~~
Deleted. The actual reward split uses integer `coverage_milli` in `node.rs`. The dead f64 functions and their two tests (`pow_pos_transition_continuous`, `reward_split_follows_stake_coverage`) were removed.

#### ~~M20: chain.base_reward always BASE_REWARD~~ — **BY DESIGN**

#### M21: Silent Borsh error handling in state root
**Location:** `account.rs:219`, `refiner.rs`
**Status:** OPEN
**What it is:** `if let Ok(bytes) = borsh::to_vec(account)` silently excludes failed serializations from the state root, causing chain divergence.
**How to fix:** Replace with `expect("Account serialization must not fail — this is a consensus bug")`.

#### M22: apply_bond refund discards errors
**Location:** `dispatcher.rs:218`
**Status:** OPEN
**What it is:** `if let Ok(()) = accounts.credit(sender, refund)` silently discards credit errors.
**How to fix:** Propagate: `accounts.credit(sender, refund).map_err(|e| format!("Bond refund failed: {}", e))?;`

#### M23: Wallet HTTP default
**Location:** `wallet/main.rs:22`
**Status:** OPEN
**What it is:** Default RPC URL is `http://localhost:4171` — signed transactions traverse network in plaintext.
**How to fix:** Change default to `https://localhost:4171` and warn on `http://` URLs.

---

### LOW (10 — L7 and L10 fixed)

#### L1: Debug derives on Bip39Mnemonic and KeyPair
**Location:** `bip39.rs:39`, `key.rs:60`
**Status:** OPEN
`#[derive(Debug)]` on both types leaks secrets to any `{:?}` formatting. Replace with manual `Debug` impls that redact secrets.

#### L2: Mnemonic printed to stdout on opl new
**Location:** `wallet/main.rs:168-169`
**Status:** OPEN
Print to stderr instead of stdout: `eprintln!("{}", mnemonic.phrase())`.

#### L3: Early-return timing in verify_ed25519
**Location:** `crypto/signing.rs:57-74`
**Status:** OPEN
Different latencies for wrong key length, invalid curve point, and bad signature. Use `subtle::ConstantTimeEq` for all comparison checks. Low priority since ed25519 verification is the dominant cost.

#### L4: Dual serialization for tx_id vs signed data
**Location:** `wallet/signing.rs:46,138-151`
**Status:** OPEN (deferred — breaking change)
tx_id uses hex-encoded sender; signature uses raw-byte sender. Two different serialization formats increase attack surface. Unify to use Borsh for both. Schedule for mainnet launch.

#### L5: Hex-encoded sender in tx_id
**Location:** `wallet/signing.rs:145`
**Status:** OPEN (deferred — breaking change)
`sender.0.to_hex().as_bytes()` converts 32 bytes to 64 bytes before hashing. Inconsistent with raw-byte hashing elsewhere. Replace with `sender.0.as_bytes()`. Breaking change — schedule for mainnet.

#### L6: No SLIP-0010 reference test vectors
**Location:** `wallet/bip39.rs:199-277`
**Status:** OPEN
Add official SLIP-0010 ed25519 test vectors as test cases. Verify derived keys match expected public keys.

#### ~~L7: Gossip max message (5 MiB) vs block max (10 MiB)~~ — **FIXD** (07da54b)
**Location:** `constants.rs:210`
**Status:** **FIXD**
`GOSSIP_MAX_MESSAGE_SIZE_BYTES` now equals `MAX_BLOCK_SIZE_BYTES` (10 MiB). Any valid block can be gossiped.

#### L8: Challenge protocol doesn't bind to PeerId
**Location:** `challenge.rs`, `main.rs:996-1022`
**Status:** OPEN
Challenge hash doesn't include PeerId, allowing precomputed answers. Fix: `blake3(format!("{}:{}", peer_id, nonce))`.

#### L9: Bip39Mnemonic::generate() panics on entropy failure
**Location:** `wallet/bip39.rs:47-48`
**Status:** OPEN
`.expect()` on OsRng can panic. Change return type to `Result<Self, WalletError>`.

#### ~~L10: Stale comments say 1,024 blocks/epoch~~ — **FIXD** (07da54b)
**Location:** `difficulty.rs`, `refiner.rs`, `pow.rs`, `constants.rs`
**Status:** **FIXD**
All comments updated from "1,024 blocks/epoch" to "960 blocks/epoch". Test parameters also updated.

---

## 25. Implementation Plan — Pass 1

### Phase A: Rename — ✓ DONE (commit ec0df9b)

1. ✓ Full validator → refiner rename across all crates, RPC, CLI, docs
2. ✓ `ConsensusPhase` enum deleted entirely (implicit phase)
3. ✓ `--validate` → `--refine` CLI flag
4. ✓ `validator_signature` → `refiner_signature`
5. ✓ `ValidatorBond/Unbond` → `RefinerBond/Unbond`
6. ✓ `ValidatorInfo/Set/Status` → `RefinerInfo/Set/Status`
7. ✓ `graduated_slash()` → `slash_refiner()` (100% burn on any double-sign)
8. ✓ `slash_offense_count` and `last_slash_height` deleted from `RefinerInfo`
9. ✓ `pos.rs` → `refiner.rs`, all module references updated
10. ✓ All variable names: `validators` → `refiners`, `validator_id` → `refiner_id`, etc.
11. ✓ All tests updated with refiner terminology (97 consensus tests passing)
12. ✓ Clean build, no stale references. All workspace tests passing.

### Phase B: Security Fixes (CRITICAL)

13. C1: Testnet/mainnet data directory isolation + chain_id check on load
14. C2: Mnemonic from `OPOLYS_MNEMONIC` env var or stdin prompt (using `rpassword`)
15. C3: `zeroize` crate on `DerivedSeed`/`KeyPair`; manual `Debug` impls that redact
16. C4+M7: RPC API key defaults to random-generated (printed at startup); constant-time comparison via `subtle::ConstantTimeEq`

### Phase C: Protocol Fixes (consensus behavior changes)

17. ✓ Delete `ConsensusPhase` from `ChainState` and `PersistedChainState` (done in ec0df9b)
18. ✓ Refiner loop: produce after `BLOCK_TARGET_TIME_MS` (90,000ms) with no miner block (done in ec0df9b)
19. M3/M4: Mutual exclusivity check in `validate_block()`
20. ✓ M1/M2: Replace `graduated_slash` with 100% burn on any double-sign (done in ec0df9b, merged with Phase A)
21. M6: Zero-miner-id — reject blocks with zero producer at height > 0
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
39. ~~M19: Delete dead `compute_pow_share`/`compute_pos_share` f64 functions~~ — **DONE** (2cf09c2)

### Phase F: Low / Cleanup

40. L1: Remove `#[derive(Debug)]` from `Bip39Mnemonic` and `KeyPair`; manual `Debug` impls that redact
41. L3: Constant-time `verify_ed25519` via `subtle`
42. L4/L5: Unify tx_id serialization (deferred — breaking change, schedule for mainnet)
43. L6: Add SLIP-0010 reference test vectors
44. L7: Raise gossip max message to match `MAX_BLOCK_SIZE_BYTES`
45. L8: Challenge protocol bind to PeerId
46. L9: `Bip39Mnemonic::generate()` → return `Result`
47. L10: Update stale comments saying 1,024 blocks/epoch → 960

### Phase G: Pass 2 (After Pass 1 is tested and working)

47. Attestation struct and `opolys/attestation/v1` P2P topic
48. Attestation collection in block builder
49. Attestation verification in `apply_block`
50. Reliability score: `consecutive_correct_attestations` in `RefinerInfo`
51. Attestation weight in reward distribution
52. Block confidence score derived on-chain
53. `opl_getBlockConfidence` RPC endpoint

---

## 26. Recent Changes

### ec0df9b — Refiner Rename & Protocol Simplification (Phase A)

**Completed Phase A of Pass 1.** All 12 items done, all 97 consensus tests passing, all workspace tests passing.

**What changed:**

1. **ConsensusPhase deleted** — There is no explicit PoW/PoS phase switch. Refiners produce blocks only after `BLOCK_TARGET_TIME_MS` (90 seconds) passes with no miner block. The reward split is continuous via `coverage_milli`; no threshold, no governance. Removed from: `types.rs`, `ChainState`, `PersistedChainState`, `ChainInfo` (RPC), `ChainInfoResponse` (RPC), genesis state hash, and all references.

2. **100% slashing** — Any double-sign burns 100% of stake immediately. No graduated penalties (10%/33%/100%), no offense counter (`slash_offense_count`), no reset window (`last_slash_height`). The old `graduated_slash()` function replaced by `slash_refiner()`. Deleted `scale_entries()` dead code.

3. **Timeout-based refiner block production** — The refiner loop in `main.rs` now checks whether chain height advanced during the sleep window. If a miner (or peer) produced a block, the refiner skips. This replaces the old phase-check logic that required `ConsensusPhase::ProofOfStake`.

4. **Refiner rename complete** — All validator references renamed throughout: types, variables, CLI flags (`--refine`), RPC endpoints (`opl_getRefiners`), module file (`pos.rs` → `refiner.rs`), comments, and tests.

5. **POS_FINALITY_BLOCKS and consecutive_pos_blocks deleted** — No finality tracking until attestations are implemented in Pass 2.

6. **Remaining cleanup noted** — `produce_pos_block()` needed renaming to `produce_refiner_block()`. Done in 2cf09c2.

### 2cf09c2 — Cleanup & Terminology (post-Phase A)

**Bug M19 fixed + terminology cleanup.** All 93 consensus tests passing (2 removed — the deleted f64 functions' tests).

**What changed:**

1. **`produce_pos_block` → `produce_refiner_block`** — The last remaining `pos` function name in `node.rs` and `main.rs` has been renamed.

2. **Dead code removed (M19)** — `compute_pow_share()` and `compute_pos_share()` (f64 functions in `emission.rs`) were never called from production code. The actual reward split uses integer `coverage_milli` arithmetic in `node.rs`. Both functions and their tests (`pow_pos_transition_continuous`, `reward_split_follows_stake_coverage`) deleted.

3. **`finalized_height` annotated as placeholder** — Added comments on the `finalized_height` field in `ChainState`, `PersistedChainState`, and `ChainInfoResponse` noting it is always 0 until finality via attestations is implemented (Pass 2).

4. **Terminology sweep** — All remaining "PoS block"/"PoW block" comments updated to "refiner block"/"mined block". Variable names `pow_share`/`pos_share` renamed to `miner_share`/`refiner_share`.

### Full Formula Audit (post-2cf09c2)

A comprehensive audit of all consensus-critical formulas and constants in the codebase against THE_PLAN.md was performed. The following updates were made to THE_PLAN.md:

- **Added to Section 3 (Constants):** `MAINNET_CHAIN_ID`, `MAX_TRANSACTIONS_PER_BLOCK`, `MAX_BLOCK_SIZE_BYTES`, `MAX_TX_DATA_SIZE_BYTES`, `MAX_FUTURE_BLOCK_TIME_SECS`, `MAX_SLASH_EVIDENCE_PER_BLOCK`, `GENESIS_DIFFICULTY`, plus full Mempool Constants subsection (`MEMPOOL_MAX_SIZE_BYTES`, `MEMPOOL_MAX_TXS_PER_ACCOUNT`, `MEMPOOL_TX_EXPIRY_SECS`, `TX_MAX_SIZE_BYTES`, `MAX_NONCE_GAP`), Mempool Congestion Pricing table, Same-Nonce Replacement rule, and Networking Constants subsection (`MAX_INBOUND_CONNECTIONS`, `MAX_OUTBOUND_CONNECTIONS`, `MAX_PEER_COUNT`, `SYNC_MAX_BLOCKS_PER_REQUEST`, `SYNC_MAX_HEADERS_PER_REQUEST`, `SYNC_REQUEST_TIMEOUT_SECS`, `SYNC_PARALLEL_PEER_COUNT`, `KAD_BUCKET_SIZE`, `PING_INTERVAL_SECS`, `PING_TIMEOUT_SECS`, `GOSSIP_MAX_MESSAGE_SIZE_BYTES`).

- **Added to Section 23 (Key Formulas):** Genesis block reward = 0, State root computation formula, Mempool Congestion Pricing tiers, Same-Nonce Replacement formula (10% fee bump), Nonce Gap Limit (`MAX_NONCE_GAP = 10`), Ban Escalation table (1h/24h/7d/permanent), Zero-elapsed-time difficulty spike (4× safety multiplier when `actual_time == 0`).

- **Added to Section 13 (Block Structure):** Transaction Root and State Root subsections describing their computation.

- **Added to Section 22 (Validation):** Mempool Inclusion Rules subsection (fee minimum, nonce gap, same-nonce replacement, per-account limit, expiry).

- **Added L10:** Stale comment bug — `difficulty.rs:5` and `constants.rs:17` say "1,024 blocks/epoch" but `EPOCH = 960`.

- **Fixed:** Section 8 header "PoS vs PoW Block Reward" → "Refiner vs Miner Block Reward". Key Formulas `pow_share_amount`/`pos_share_amount` → `miner_share_amount`/`refiner_share_amount`. PING_TIMEOUT_SECS corrected from 10s to 20s. Section numbering updated after merging old Section 23 (New Constants) into Section 3.

- **Confirmed discrepancies (bugs already tracked):** H3 (suggested fee uses `total_fees` not `total_fees_burned`), M4 (missing mutual exclusivity check), H4 (unbond fee bypass), M6 (zero producer phantom issuance), M18 (integer division bias in retarget), H6 (no sync response size limit).

---

*This document is the single source of truth for Opolys development. Update it with every design decision and implementation change.*