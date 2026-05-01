# Opolys — Decentralized Digital Gold

**OPL** is decentralized digital gold. A pure coin blockchain where every parameter emerges from mathematics or market forces — no tokens, no assets, no governance, no schedules, no caps. Just a coin, mined like gold, held like gold, spent like gold.

---

## Why Gold?

Gold has been humanity's store of value for 5,000 years. It works because of physics, not policy. Nobody votes on how much gold is in the earth. Nobody sets a production schedule. Nobody reverses a transaction. The harder gold is to find, the less of it enters circulation. Jewelry gets lost, coins get melted — the stock slowly shrinks. Gold's value doesn't come from scarcity alone; it comes from the *cost* of finding more of it and the *permanent attrition* of what already exists.

Opolys encodes these properties directly into consensus:

| Gold Property | Opolys Equivalent | How It Works |
|---|---|---|
| **Gold mining gets harder over time** | Difficulty rises as more OPL is mined | Miners must find hashes with more leading zero bits as the network grows. More hash power → faster blocks → difficulty increases → each block yields less OPL, just like real veins depleting. |
| **Gold supply has no cap** | No maximum supply | There is no "21 million" moment. OPL issuance naturally declines as difficulty rises, but it never reaches zero. Like gold — there's always a little more to be found, it just costs more. |
| **Gold is lost over time** | All transaction fees are burned | Every fee permanently destroys OPL from circulation. Shipwrecks, lost jewelry, melted coins — Opolys models this as fee burning. The circulating supply can *shrink*. |
| **Gold mining is a physical process** | EVO-OMAP memory-hard proof-of-work | Mining requires 256 MiB of memory and data-dependent computation. No shortcut, no ASIC cheat. Like digging a shaft — you have to move the rock. |
| **Gold ore varies in richness** | Vein yield: `1 + ln(target / hash_int)` | A lucky gold strike yields more than a poor one. Vein yield models this: most blocks earn ~2x base reward, exceptional ones earn more. The math is natural, not scheduled. |
| **Gold production rate is known** | BASE_REWARD = 332 OPL (testnet), derived from world gold production | 3,630 tonnes of gold are mined annually (~116.7 million troy ounces). Divided by 350,640 blocks per year = 332 OPL per block at minimum difficulty. Mainnet BASE_REWARD is set from live data at genesis ceremony. |
| **Gold must be refined before use** | Difficulty must be overcome to earn reward | You can't just claim gold exists — you have to prove you did the work. EVO-OMAP requires a valid proof-of-work with at least D leading zero bits. |
| **Gold held in vaults earns trust** | Validator staking with seniority | Bonded OPL gives validators block production rights. Senior validators earn slightly more (logarithmic weight), just as trusted vaults command higher fees. But the marginal bonus shrinks over time — no permanent aristocracy. |
| **Gold can be unvaulted** | FIFO unbonding with 1-epoch delay | Unbonding OPL is like withdrawing gold from a vault. It takes time (960 blocks = exactly 24 hours). During the delay, you still earn rewards. The oldest deposits are withdrawn first. |
| **Gold bars are uniform** | Every OPL is identical. One sub-unit (Flake). No tokens, no assets, no governance tokens | There's no "pennyweight" gold or "grain" gold in Opolys. 1 OPL = 1,000,000 Flakes. Period. The chain tracks one asset. |
| **Gold supply is self-regulating** | Natural equilibrium — no governance needed | When fees are burned faster than rewards are issued, supply shrinks. When mining is too easy, difficulty rises and issuance drops. The protocol never needs a vote. |

**TL;DR**: If Bitcoin is digital gold with a cap, Opolys is digital gold *without* a cap — because real gold doesn't have one either. The value comes from the cost of production, not from a supply schedule.

---

## The Gold Derivation

The testnet base block reward of 332 OPL is not an arbitrary number. It comes directly from real-world gold production data:

```
Annual gold production           ≈ 3,630 tonnes          (USGS/WGC 2024-2025)
Convert to troy ounces           ≈ 116,707,041 oz        (3,630 × 32,150.7)
Blocks per year                  = 350,640                (365.25 × 86,400 / 90)
BASE_REWARD (testnet)            = floor(116,707,041 / 350,640) = 332 OPL
```

For **mainnet**, `BASE_REWARD` is not hardcoded — it is derived during the genesis ceremony from live LBMA/USGS/WGC data at launch time and embedded in the genesis block attestation. This anchors the supply model to real gold market data on the exact day the network starts.

At difficulty 1 (minimum), each block earns 332 OPL. As difficulty rises, the per-block reward naturally shrinks — `block_reward = (BASE_REWARD / difficulty) × vein_yield`. This mirrors real gold: the easy veins are found first, and every subsequent ounce costs more to extract.

The block time of 90,000 ms (90 seconds) is chosen so that exactly 960 blocks complete in 24 hours:

```
960 × 90,000 ms = 86,400,000 ms = 86,400 seconds = 24 hours
```

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

### Run a Testnet Node

The fastest way to test Opolys locally:

```bash
# One-command testnet (builds node, generates miner key, starts mining)
./scripts/testnet-bootstrap.sh          # Start with defaults
./scripts/testnet-bootstrap.sh --reset  # Reset chain data and start fresh
```

Three genesis accounts are pre-funded with 10,000 OPL each.
Testnet keys are at: `testnet-data/testnet-keys.txt`

```bash
# Manual testnet start
cargo run --release -- --testnet --mine --validate --key-file testnet-data/miner.key

# Testnet with debug logging and custom ports
cargo run --release -- --testnet --mine --port 5000 --rpc-port 5001 --log-level debug

# Testnet isolated (no bootstrap peers)
cargo run --release -- --testnet --mine --no-bootstrap
```

### Run a Mainnet Node

Mainnet requires a genesis ceremony attestation file:

```bash
# Run the genesis ceremony to generate attestation
cargo run --bin genesis-ceremony -- \
  --gold-price-usd-cents 328700 \
  --annual-production-tonnes 3630 \
  --above-ground-tonnes 219891 \
  --output genesis-params.json

# Start mainnet node with ceremony output
cargo run --release -- \
  --genesis-params genesis-params.json \
  --mine \
  --key-file /path/to/miner.key \
  --rpc-api-key <secret>
```

### CLI Flags

| Flag | Default | Description |
|---|---|---|
| `--port` | 4170 | P2P listen port |
| `--rpc-port` | 4171 | JSON-RPC server port |
| `--data-dir` | `./data` | RocksDB storage directory |
| `--bootstrap` | _(none)_ | Bootstrap peer address(es), comma-separated |
| `--no-bootstrap` | disabled | Skip DNS seeds and peer cache; only dial `--bootstrap` peers |
| `--log-level` | `info` | Log level: trace, debug, info, warn, error |
| `--mine` | disabled | Enable PoW mining loop |
| `--validate` | disabled | Enable PoS block production |
| `--key-file` | _(none)_ | Path to 32-byte ed25519 seed file |
| `--testnet` | disabled | Pre-funded genesis accounts (DO NOT use in production) |
| `--genesis-params` | _(none)_ | Path to genesis ceremony JSON (required for mainnet) |
| `--no-rpc` | disabled | Disable JSON-RPC server |
| `--rpc-listen-addr` | `127.0.0.1` | RPC listen address (`0.0.0.0` to expose publicly) |
| `--rpc-api-key` | _(none)_ | API key for write/mining RPC methods |

### Wallet CLI (`opl`)

```bash
# Generate a new wallet (BIP-39 24-word mnemonic)
opl new

# Show wallet address
opl address

# Check balance via RPC
opl balance

# Transfer OPL
opl transfer --recipient <hex_object_id> --amount <flakes> --fee <flakes>

# Bond stake as validator
opl bond --amount <flakes> --fee <flakes>

# Unbond stake (FIFO order)
opl unbond --amount <flakes> --fee <flakes>

# Sign and broadcast a transaction
opl send --signed-tx <hex>
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

### Lint

```bash
cargo clippy --all-targets --all-features
cargo fmt --check
```

---

## Currency

OPL uses exactly **6 decimal places**. There is one unit and one sub-unit:

| Unit | Flakes | Example |
|---|---|---|
| **OPL** | 1,000,000 | `1.000000 OPL` |
| **Flake** | 1 | `0.000001 OPL` |

All on-chain arithmetic uses `FlakeAmount` (u64) — no floating point anywhere in consensus. Display formatting always shows 6 decimal places: `312.000000 OPL`, `0.000001 OPL`.

The name "Flake" comes directly from gold: a flake is the smallest piece of gold you can hold. In Opolys, one Flake is the smallest indivisible unit of account — just as one grain of gold is the smallest practical weight.

---

## Architecture

```
Opolys/
├── Cargo.toml                                        # Workspace
├── THE_PLAN.md                                       # Single source of truth
├── scripts/testnet-bootstrap.sh                      # One-command testnet launcher
├── crates/
│   ├── core/          — Shared types, constants, errors
│   │   ├── constants.rs     # BASE_REWARD, EPOCH, BLOCK_TARGET_TIME_MS, etc.
│   │   ├── types.rs         # Hash, ObjectId, Transaction, Block, BlockHeader
│   │   └── errors.rs        # OpolysError enum
│   ├── crypto/        — Blake3-256, SHA3-256, ed25519
│   │   ├── hash.rs           # Blake3-256, Blake3 XOF, SHA3-256
│   │   ├── signing.rs        # ed25519 verification
│   │   └── key.rs            # KeyPair
│   ├── consensus/     — Consensus engine
│   │   ├── account.rs        # AccountStore with fee-burning transfers
│   │   ├── block.rs          # compute_block_hash(), compute_transaction_root()
│   │   ├── difficulty.rs     # Adaptive retarget, consensus floor, PoW check
│   │   ├── emission.rs       # Vein yield, difficulty_to_target(), suggested_fee, stake_coverage
│   │   ├── mempool.rs        # Fee-priority mempool
│   │   ├── pos.rs            # ValidatorSet, FIFO unbonding, seniority weights
│   │   ├── pow.rs            # EVO-OMAP PowContext, mine_parallel, verify_light
│   │   └── genesis.rs        # GenesisConfig, testnet_genesis_config()
│   ├── execution/     — Transaction dispatcher (Transfer, Bond, Unbond)
│   │   └── dispatcher.rs      # verify_transaction(), apply_transaction()
│   ├── storage/       — RocksDB persistence
│   │   └── store.rs          # BlockchainStore, PersistedChainState
│   ├── networking/     — libp2p P2P networking
│   │   ├── behaviour.rs       # OpolysBehaviour (gossipsub+kad+identify+ping+request-response)
│   │   ├── network.rs         # Swarm, event routing, NetworkCommand
│   │   ├── gossip.rs          # GossipConfig (tx/block topics)
│   │   ├── sync.rs            # SyncRequest/SyncResponse (CBOR)
│   │   └── discovery.rs       # DiscoveryConfig (Kademlia DHT)
│   ├── wallet/         — Wallet CLI (`opl` binary)
│   │   ├── bip39.rs           # BIP-39 mnemonic + SLIP-0010 derivation
│   │   ├── signing.rs         # TransactionSigner with signature_type
│   │   ├── key.rs             # KeyPair
│   │   └── account.rs         # AccountInfo
│   ├── rpc/            — Axum JSON-RPC 2.0 server
│   │   └── server.rs         # MiningJobResponse, RpcState, all endpoints
│   └── node/           — Full node orchestration
│       ├── main.rs            # CLI, event loop, P2P wiring
│       └── node.rs            # OpolysNode, ChainState, apply_block, mining loop
```

### Crate Dependencies

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
| `sha3` | 0.10 | EVO-OMAP finalization (SHA3-256) |
| `ed25519-dalek` | 2.1 | Transaction and block signing |
| `rocksdb` | 0.22 | Persistent storage |
| `libp2p` | 0.54 | P2P networking (QUIC, Kademlia, Gossipsub) |
| `tokio` | 1 | Async runtime |
| `axum` | 0.8 | JSON-RPC server |
| `evo-omap` | local | Proof-of-work algorithm (EVO-OMAP) |
| `rayon` | 1.10 | Parallel mining |
| `bip39` | 2.2 | Mnemonic generation |

---

## Cryptographic Stack

| Layer | Algorithm | Purpose |
|---|---|---|
| **Hashing** | Blake3-256 (32 bytes) | Block hashes, transaction IDs, ObjectIds, state roots, Merkle roots |
| **PoW Inner** | Blake3 (XOF mode) | EVO-OMAP dataset generation, branch mixing |
| **PoW Final** | SHA3-256 | EVO-OMAP final hash (different security margin from inner) |
| **Signing** | ed25519 (via ed25519-dalek) | Transaction authentication and validator block signing |
| **Key Derivation** | SLIP-0010 + HMAC-SHA512 | BIP-44 path: `m/44'/999'/0'/0'` |
| **Mnemonic** | BIP-39 (24-word, 256-bit entropy) | Wallet recovery |

### Single-Key Architecture

A single ed25519 keypair — derived deterministically from the BIP-39 mnemonic via SLIP-0010 — handles both transaction signing and validator block signing. Full wallet recovery is possible from the mnemonic alone. No separate validator key file is needed (though a `--key-file` flag can load a raw seed for convenience).

### ObjectId

Account addresses are **Blake3-256 hashes of ed25519 public keys** — not the public keys themselves. This provides a 32-byte uniform address space and an extra hash layer for privacy (the public key is only revealed when the account sends its first transaction).

### Planned Cryptography

| Layer | Algorithm | Purpose |
|---|---|---|
| **Validator Signatures** | BLS12-381 | Signature aggregation for efficient PoS attestation |
| **Block Producer Selection** | VRF | Unpredictable, verifiable validator selection |
| **Privacy (L1)** | Stealth addresses | Receiver privacy via one-time derived addresses |
| **Privacy (L2)** | Viewing keys | Selective transaction disclosure |
| **ZK Foundation** | Poseidon hash | ZK-friendly hash for future SNARKs/STARKs |

---

## Consensus

Opolys uses **hybrid PoW/PoS** with a smooth, continuous transition — no thresholds, no governance votes, no hard switches.

### How It Works

1. **Miners compete** to find EVO-OMAP proof-of-work solutions (like physical gold miners)
2. **Validators bond stake** and earn the right to produce blocks proportional to bonded weight (like gold vaults earning trust)
3. **Stake coverage** (`bonded_stake / total_issued`) continuously shifts rewards from miners to validators
4. At 0% coverage, 100% of rewards go to miners. At 100% coverage, 100% go to validators. The split is smooth and mathematical — no vote needed

### Difficulty

EVO-OMAP difficulty D means the SHA3-256 hash must have **at least D leading zero bits**. This is NOT a u64-target divisor model.

```
target = 2^(64-D) - 1       where D = difficulty (leading zero bits)
valid if: hash_value_u64 ≤ target
```

- At difficulty 1: target = 2^63 - 1 (roughly half of all u64 values pass)
- At difficulty 10: target = 2^54 - 1 (about 1 in 1,024 values pass)
- At difficulty 20: target = 2^44 - 1 (about 1 in 1 million values pass)

### Difficulty Retargeting

Every `EPOCH` (960 blocks = exactly 24 hours):

```
new_difficulty = old_difficulty × expected_time_ms / actual_time_ms
```

- If blocks arrived too fast → difficulty increases
- If blocks arrived too slow → difficulty decreases
- **No maximum clamp** — difficulty adjusts freely
- The only floor is `MIN_DIFFICULTY` (1), which is a mathematical requirement, not an arbitrary cap

### Effective Difficulty

```
effective_difficulty = max(retarget, consensus_floor, MIN_DIFFICULTY)
```

The **consensus floor** is `total_issued / bonded_stake`. As more OPL enters circulation relative to bonded stake, difficulty cannot fall below this organic floor. This prevents an attacker from dropping difficulty by unbonding stake.

### Proof of Work: EVO-OMAP

EVO-OMAP (EVOlutionary Oriented Memory-hard Algorithm for Proof-of-work) is the mining algorithm:

| Property | Value | Gold Analogy |
|---|---|---|
| Memory footprint | 256 MiB | You can't mine gold without a pickaxe |
| Memory access pattern | Read-write per step | You have to move the rock, not just look at it |
| Branch factor | 4-way | The shaft doesn't go in a straight line |
| Execution model | Superscalar (8 instructions/step) | Real mining is complex, not just repetitive hashing |
| State size | 512 bits | Your claim on a vein is specific, not generic |
| Dataset | 256 MiB, chained | Can't precompute — you have to dig in real time |
| Inner hash | Blake3 XOF | Fast inner loop |
| Final hash | SHA3-256 | Different security assumption from inner loop |
| Rotation | ROTL/ROTR memory-dependent | The ore deposits determine which way the tunnel turns |
| Integer-only | Yes — no floating point in consensus | Gold mining is physical, not simulated |

Mining API:

```rust
// Multi-threaded mining (uses rayon)
let (nonce, attempts) = mine_parallel(header, height, difficulty, max_attempts, num_threads);

// Full verification (requires 256 MiB dataset cache)
let valid = verify(header, height, nonce, difficulty);

// Light verification (on-demand node reconstruction, no 256 MiB allocation)
let valid = verify_light(header, height, nonce, difficulty);
```

Block validation uses `verify_light()` to avoid allocating 256 MiB on every block. Mining uses the cached dataset via `PowContext`.

### Vein Yield

Gold veins vary in richness. Opolys models this with vein yield:

```
vein_yield = 1 + ln(target / hash_int)
```

Where:
- `target = 2^(64-D) - 1` (from difficulty D)
- `hash_int` = first 8 bytes of the EVO-OMAP PoW hash, interpreted as big-endian u64

Most blocks earn ~2x BASE_REWARD. Exceptionally lucky blocks earn more. The math is natural: `ln(x)` is the same curve that describes ore concentration in a gold vein. Implementation uses `f64::ln()` with deterministic IEEE 754 rounding — identical results across all platforms.

### Block Reward Formula

```
block_reward = (BASE_REWARD / effective_difficulty) × vein_yield
```

At minimum difficulty (1), each block earns ~312 OPL. As difficulty rises, the per-block reward naturally declines — exactly like real gold mining where the easy veins are found first.

### Reward Distribution (PoW/PoS Split)

```
coverage_milli = (bonded_stake × 1000) / total_issued    // integer, no float
pow_share_amount = block_reward × (1000 - coverage_milli) / 1000
pos_share_amount = block_reward - pow_share_amount
```

- PoW share goes to the block producer (miner)
- PoS share is distributed among active validators proportional to their weight
- At 0% coverage: 100% miner, 0% validators
- At 100% coverage: 0% miner, 100% validators
- This is the same continuum as gold: as more gold moves from mines to vaults, the vaults command more influence

### Suggested Fee (EMA)

```
suggested_fee = (current_fees + 9 × previous_suggested_fee) / 10
```

Floored at `MIN_FEE` (1 Flake). Starts at 1 Flake. This is a *suggestion* — the mempool accepts any fee ≥ 1 Flake. Markets set the real price.

---

## Validator Staking

### Bond Lifecycle

Like depositing gold in a vault — you lock it up, it earns seniority, and you can withdraw it after a delay.

1. **Bond**: `ValidatorBond { amount }` — Lock OPL as validator stake. Creates a new bond entry if the validator already exists (top-up)
2. **Unbond**: `ValidatorUnbond { amount }` — Withdraw OPL using FIFO order (oldest first)
3. **Slash**: Only for double-signing. All entries' stakes are **burned** (not confiscated). This is the only slashing condition

### Per-Entry Weight (Seniority)

```
entry_weight = stake × (1 + ln(1 + age_years))
```

Each bond entry has its own seniority clock. Logarithmic seniority means older entries earn more per-coin, but the marginal bonus shrinks over time — no permanent aristocracy, just like real gold where established vaults command more trust but new entrants are never locked out.

### FIFO Unbonding

```
ValidatorUnbond { amount: FlakeAmount }
```

Oldest entries are consumed first. If the unbond amount exceeds an entry's stake, that entry is fully consumed and the remainder comes from the next entry. Residuals keep their original `bonded_at_timestamp` (preserving seniority). Entries with the same `bonded_at_timestamp` are auto-merged.

After unbonding, stake enters the **unbonding queue** for `UNBONDING_DELAY_BLOCKS` (960 blocks = exactly 24 hours). During the delay, the unbonding stake still earns rewards. Once matured, it's automatically credited back to the sender.

### Validator Activation

Newly bonded validators start in `Bonding` status. They activate to `Active` after their earliest bond entry has been confirmed for at least one full epoch (960 blocks). Only `Active` validators are eligible for block production.

**Validator caps:**
- **Active set**: 5,000 validators maximum (can be raised via protocol upgrade after testnet)
- **Total registered**: up to 524,288 validators can be in `Bonding`/`Waiting` status
- New validators bond successfully and queue fairly — no `ValidatorBond` is ever rejected

### Block Producer Selection

A deterministic seed derived from the previous block hash selects the PoS block producer. Weighted random sampling from active validators. Any node can verify the selection — no trust required.

---

## Block Structure

```rust
BlockHeader {
    version: u32,                       // Protocol version (currently 1)
    height: u64,                        // 0 for genesis
    previous_hash: Hash,                 // Blake3-256 of prior block
    state_root: Hash,                    // Blake3-256 of post-execution state
    transaction_root: Hash,              // Blake3-256 commitment of tx IDs
    timestamp: u64,                      // UNIX epoch seconds
    difficulty: u64,                     // Effective difficulty (leading zero bits)
    suggested_fee: FlakeAmount,           // EMA of previous block's fees (1 Flake minimum)
    extension_root: Option<Hash>,         // Reserved for rollups
    producer: ObjectId,                   // Miner or validator earning the block reward
    pow_proof: Option<Vec<u8>>,            // EVO-OMAP nonce (None for PoS)
    validator_signature: Option<Vec<u8>>,  // ed25519 signature (None for PoW)
}

Block {
    header: BlockHeader,
    transactions: Vec<Transaction>,
}
```

### Block Hash

`Blake3-256(header_bytes)` where `pow_proof` and `validator_signature` are set to `None` before hashing. The hash is determined before mining begins. The genesis block hash is computed from ceremony parameters, not hardcoded.

### State Root

After each block, `compute_state_root()` computes `Blake3-256(sorted Borsh-serialised state)` over both accounts and validators. This root makes the full application state a single 32-byte commitment.

---

## Transactions

### Types

| Action | Description |
|---|---|
| `Transfer { recipient, amount }` | Move OPL from sender to recipient; fee is burned |
| `ValidatorBond { amount }` | Lock OPL as validator stake (new entry or top-up, min 1 OPL per new entry) |
| `ValidatorUnbond { amount }` | Withdraw OPL using FIFO order; fee is burned; 1,024-block delay |

### Transaction Structure

```rust
Transaction {
    tx_id: ObjectId,           // Blake3-256(sender || action || fee || nonce)
    sender: ObjectId,          // Blake3-256(ed25519_pubkey)
    action: TransactionAction,
    fee: FlakeAmount,           // Burned, not collected
    signature: Vec<u8>,         // ed25519 signature
    signature_type: u8,          // 0 = ed25519 (reserved for future types)
    nonce: u64,                  // Replay protection
    data: Vec<u8>,               // Arbitrary attachment (max 1 KiB)
    public_key: Vec<u8>,         // ed25519 public key (32 bytes) — Blake3(public_key) == sender must hold
}
```

### Signature Verification

1. `Blake3(public_key) == sender` — binds the key to the identity
2. `tx_id == compute_tx_id(sender, action, fee, nonce)` — transaction integrity
3. `ed25519_verify(signed_data, signature, public_key)` — authenticity

Invalid transactions (wrong nonce, insufficient balance, invalid unbond amount) result in **no fee burn and no nonce advance**.

### Fee Model

All fees are **permanently burned** — not transferred to validators or miners. This is the gold attrition model: just as gold jewelry is lost, gold coins are melted, and gold bullion sinks, OPL fees are destroyed, reducing circulating supply.

- **No minimum fee beyond 1 Flake**: The mempool accepts any transaction
- **No fee schedule**: Markets determine inclusion priority
- **Validator income**: Block rewards only, never fees
- **Deflationary**: Fee burning can make circulating supply decrease over time

---

## RPC API

JSON-RPC 2.0 server on port 4171 (default: `listen_port + 1`).

### Read

| Method | Parameters | Description |
|---|---|---|
| `opl_getBlockHeight` | _(none)_ | Current chain height |
| `opl_getChainInfo` | _(none)_ | Chain statistics (height, difficulty, supply, validators, suggested_fee) |
| `opl_getNetworkVersion` | _(none)_ | Protocol version string |
| `opl_getBalance` | `["object_id_hex"]` | Account balance (flakes and OPL) |
| `opl_getAccount` | `["object_id_hex"]` | Full account details (balance, nonce, public_key) |
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
| `opl_getMiningJob` | _(none)_ | Block template for external miners (includes `header_bytes`, `producer`, `target`) |
| `opl_submitSolution` | `["borsh_hex_string"]` | Submit mined block |

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

# Get mining job
curl -X POST http://localhost:4171/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"opl_getMiningJob","params":null,"id":4}'

# Submit transaction
curl -X POST http://localhost:4171/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"opl_sendTransaction","params":["<borsh_hex>"],"id":5}'
```

---

## Wallet Key Derivation

### BIP-44 Path

```
m / 44' / 999' / account' / 0'
│    │     │       │        └── change (always 0' for ed25519)
│    │     │       └── account index (0, 1, 2, ...)
│    │     └── SLIP-0044 coin type 999 (Opolys)
│    └── BIP-44 purpose (always 44')
└── master key (hardened)
```

### Recovery

| Key Type | Recoverable from Mnemonic? | Backup Required |
|---|---|---|
| ed25519 | **Yes** — deterministic SLIP-0010 derivation | Mnemonic phrase only |

### Mnemonic Format

- 24-word BIP-39 phrase (256 bits of entropy)
- Standard English wordlist with checksum validation
- Optional passphrase (BIP-39 password) for additional security

---

## Networking

- **Transport**: libp2p 0.54 with QUIC, TCP, noise, yamux, relay client
- **Discovery**: Kademlia DHT (bucket size 20) + identify protocol
- **Gossip**: Gossipsub for block/transaction propagation (`opolys/tx/v1`, `opolys/block/v1`)
- **Sync**: CBOR request-response protocol for block download (`/opolys/sync/1`)
- **Ping**: Liveness checks with 30s interval, 20s timeout

| Parameter | Value |
|---|---|
| `MAX_INBOUND_CONNECTIONS` | 50 |
| `MAX_OUTBOUND_CONNECTIONS` | 50 |
| `MAX_PEER_COUNT` | 200 |
| `SYNC_MAX_BLOCKS_PER_REQUEST` | 500 |
| `SYNC_MAX_HEADERS_PER_REQUEST` | 2,000 |
| `SYNC_REQUEST_TIMEOUT_SECS` | 30 |
| `SYNC_PARALLEL_PEER_COUNT` | 3 |
| `KAD_BUCKET_SIZE` | 20 |
| `PING_INTERVAL_SECS` | 30 |
| `PING_TIMEOUT_SECS` | 20 |
| `GOSSIP_MAX_MESSAGE_SIZE` | 5 MiB |

---

## Storage

RocksDB with Borsh serialization. State is saved atomically after each block.

| Column Family | Key | Value |
|---|---|---|
| `blocks` | `block_<height>` | Borsh-serialized `Block` |
| `accounts` | `account_<hex_object_id>` | Borsh-serialized `Account` |
| `validators` | `validator_<hex_object_id>` | Borsh-serialized `ValidatorInfo` |
| `chain_state` | `chain_state` | Borsh-serialized `PersistedChainState` |

---

## Block & Transaction Validation

Every block applied to the chain must pass these checks:

1. **Version** must match `BLOCK_VERSION` (currently 1)
2. **Height** must equal `parent_height + 1`
3. **Previous hash** must match parent's hash (`Hash::zero()` for genesis)
4. **Timestamp** must be strictly greater than parent, within 5 minutes of wall clock
5. **Difficulty** must match the expected next difficulty from retargeting
6. **Transaction count** must not exceed `MAX_TRANSACTIONS_PER_BLOCK` (10,000)
7. **Block size** must not exceed `MAX_BLOCK_SIZE_BYTES` (10 MiB)
8. **Transaction root** must match `compute_transaction_root()`
9. **No duplicate transactions** (each `tx_id` must be unique within the block)
10. **Transaction data** must not exceed `MAX_TX_DATA_SIZE_BYTES` (1 KiB)
11. **Fee minimum**: each transaction fee must be at least `MIN_FEE` (1 Flake)
12. **PoW proof**: for PoW blocks, the EVO-OMAP proof must satisfy the difficulty target
13. **PoS signature**: for PoS blocks, the validator signature must be valid and the producer must match deterministic selection

---

## Constants Reference

| Constant | Value | Description |
|---|---|---|
| `CURRENCY_NAME` | `"Opolys"` | Human-readable name |
| `CURRENCY_TICKER` | `"OPL"` | Exchange ticker |
| `CURRENCY_SMALLEST_UNIT` | `"Flake"` | Name of 1/1,000,000 OPL |
| `FLAKES_PER_OPL` | 1,000,000 | Fundamental unit ratio |
| `DECIMAL_PLACES` | 6 | Always 6 decimal places |
| `BASE_REWARD` | 332,000,000 Flakes (332 OPL) testnet; mainnet from genesis ceremony | Gold-derived block reward base |
| `MIN_DIFFICULTY` | 1 | Mathematical floor (not a cap) |
| `EPOCH` | 960 blocks | Unified epoch for retarget, dataset regen, unbonding (= exactly 24 hours) |
| `UNBONDING_DELAY_BLOCKS` | 960 | One epoch delay for unbonding |
| `MIN_FEE` | 1 Flake | Floor for market-driven fees |
| `MIN_BOND_STAKE` | 1,000,000 Flakes (1 OPL) | Minimum per new bond entry |
| `BLOCK_VERSION` | 1 | Current protocol version |
| `SIGNATURE_TYPE_ED25519` | 0 | ed25519 signature type |
| `EXTENSION_TYPE_NONE` | 0 | No extension data |
| `EXTENSION_TYPE_ROLLUP` | 1 | Rollup data (reserved) |
| `POS_FINALITY_BLOCKS` | 3 | PoS finality depth |
| `BLOCK_TARGET_TIME_MS` | 90,000 | 90 seconds per block |
| `BLOCK_TARGET_TIME_SECS` | 90 | 90 seconds per block |
| `MAX_ACTIVE_VALIDATORS` | 5,000 | Active validator set cap |
| `NETWORK_PROTOCOL_VERSION` | `"1.0.0"` | Protocol identifier |
| `DEFAULT_LISTEN_PORT` | 4170 | P2P listen port |
| `MAX_TRANSACTIONS_PER_BLOCK` | 10,000 | Max transactions per block |
| `MAX_BLOCK_SIZE_BYTES` | 10,485,760 (10 MiB) | Max block size |
| `MAX_TX_DATA_SIZE_BYTES` | 1,024 (1 KiB) | Max transaction data field |
| `MAX_FUTURE_BLOCK_TIME_SECS` | 300 (5 min) | Max clock skew for block timestamp |
| `MEMPOOL_MAX_SIZE_BYTES` | 100 MiB | Max mempool memory |
| `MEMPOOL_MAX_TXS_PER_ACCOUNT` | 50 | Max pending txs per account |
| `MEMPOOL_TX_EXPIRY_SECS` | 86,400 (24h) | Mempool transaction expiry |
| `GOSSIP_MAX_MESSAGE_SIZE_BYTES` | 5 MiB | Max gossip message size |

---

## Key Formulas

### Block Reward
```
vein_yield = 1 + ln(target / hash_int)              // f64, rounded to nearest milli
block_reward = (BASE_REWARD / effective_difficulty) × vein_yield
```

### Effective Difficulty
```
effective_difficulty = max(retarget, consensus_floor, MIN_DIFFICULTY)
```

### Difficulty Retarget
```
new_difficulty = old_difficulty × expected_time_ms / actual_time_ms
```
No maximum clamp. Floor is MIN_DIFFICULTY (1).

### Consensus Floor
```
consensus_floor = total_issued / bonded_stake
```

### EVO-OMAP Difficulty Target
```
target = 2^(64-D) - 1    where D = difficulty (leading zero bits)
valid if: u64(pow_hash[..8]) ≤ target
```

### Suggested Fee
```
suggested_fee = (current_fees + 9 × previous_suggested_fee) / 10, floored at MIN_FEE
```

### Validator Weight
```
entry_weight = stake × (1 + ln(1 + age_years))
```

### Stake Coverage & Reward Split
```
coverage_milli = (bonded_stake × 1000) / total_issued        // integer, no float
pow_share_amount = block_reward × (1000 - coverage_milli) / 1000
pos_share_amount = block_reward - pow_share_amount
```

---

## Genesis Ceremony

The genesis block is created from a `GenesisAttestation` containing:

- **ceremony_timestamp**: UNIX timestamp of the ceremony
- **gold_spot_price_usd_cents**: LBMA gold price at ceremony time
- **annual_production_tonnes**: USGS annual mine production (~3,630 t)
- **total_above_ground_tonnes**: WGC above-ground stock (~219,891 t)
- **Response hashes**: Blake3-256 hashes of the raw LBMA, USGS, and WGC responses
- **Derivation formula**: The mathematical formula linking gold data to BASE_REWARD

The genesis block has height 0, zero previous hash, no transactions, no PoW proof, and a state root computed from ceremony parameters and protocol constants.

In `--testnet` mode, three deterministic testnet accounts are pre-funded with 10,000 OPL each for testing.

---

## Security Features

Opolys implements layered P2P defenses to protect honest nodes from adversarial peers:

| Feature | What It Does |
|---|---|
| **Eclipse attack protection** | Mining waits for ≥3 outbound peers before starting. All peers being attacker-controlled cannot force a fake chain on a miner. |
| **Subnet diversity** | Max 3 peers per /24 IPv4 subnet. Prevents geographic concentration of adversarial peers from a single AS/datacenter. |
| **Memory-hard challenge** | Every new Opolys peer must pass an EVO-OMAP memory challenge before gossip is accepted. Prevents Sybil flooding with lightweight fake nodes. |
| **Immediate permanent ban** | Fake PoW blocks and invalid ed25519 signatures trigger permanent bans — zero tolerance for cryptographically invalid data. |
| **Graduated strike system** | Anonymous peers banned after 2 invalid blocks; known validators after 3. Escalating ban durations (1h → 24h → 7d → permanent). |
| **Fake PoW pre-check** | Vein yield pre-check filters gossip blocks before expensive `apply_block()` lock acquisition, preventing cheap CPU-waste attacks. |
| **Wrong chain_id ban** | Transactions from a different chain (replay attacks) result in 24h ban. |
| **Rate limiting** | Per-peer gossip rate limits: 10 blocks/sec and 50 txs/sec (doubled for known validators). |
| **Fee-weighted relay** | Low-fee transactions are delayed 5 seconds before P2P relay to reduce low-cost mempool spam. |
| **Mempool DoS protection** | 100 MiB cap, 50 tx/account limit, 24h expiry, nonce-gap filtering, epoch-boundary eviction. |
| **Persistent ban list** | Bans survive node restarts (stored in `data/banned_peers.json`). |

---

## What Opolys Is Not

- **Not a smart contract platform** — no WASM, no VM, no object model. The chain tracks one asset: OPL.
- **Not a multi-asset chain** — no tokens, no NFTs, no colored coins. One coin, one purpose.
- **Not governed** — no on-chain governance, no voting, no committees. Parameters emerge from math.
- **Not scheduled** — no halvings, no emission calendar. Difficulty and rewards emerge from chain state.
- **Not capped** — no maximum supply. OPL issuance naturally declines as difficulty rises, like real gold mining.
- **Not reversible** — no reversal windows. Only double-signing gets slashed. Finality is final.

---

## Design Principles

| Principle | Detail |
|---|---|
| **No hard cap** | Supply grows via block rewards; fees are burned, modeling real gold attrition |
| **No governance** | No on-chain governance, no voting, no committees |
| **No schedules** | Difficulty and rewards emerge from chain state, not from a calendar |
| **No hardcoded fees** | Fees are market-driven and burned entirely |
| **Only double-signing slashed** | No reversal windows, no confiscation for any other reason |
| **Gold-derived emission** | BASE_REWARD = 312 OPL, derived from annual gold production (~3,630 tonnes) |
| **Integer-only consensus** | No floating-point arithmetic in consensus-critical code (except `f64::ln()` for vein yield, which is IEEE 754 deterministic) |
| **Single key** | One ed25519 key for both transactions and validation, derived from BIP-39 mnemonic |
| **Core only** | The node is the protocol layer (like Bitcoin Core). Community builds explorers, wallets, and mining pools |

---

## Roadmap

| Phase | Status | Description |
|---|---|---|
| Core types & constants | **DONE** | Hash, ObjectId, Transaction, Block, all constants |
| Crypto | **DONE** | Blake3, SHA3-256, ed25519, key derivation |
| Consensus engine | **DONE** | EVO-OMAP PoW, vein yield, difficulty, FIFO unbonding, PoS selection |
| Storage | **DONE** | RocksDB with all column families, atomic state saves |
| Execution | **DONE** | Transaction dispatcher (Transfer, Bond, Unbond) with fee burning |
| Wallet | **DONE** | BIP-39, SLIP-0010, ed25519 signing, CLI (`opl`) |
| RPC | **DONE** | JSON-RPC 2.0 with mining endpoints, chain queries, API key auth |
| Node | **DONE** | Full node with mining loop, block application, P2P event loop |
| Networking | **DONE** | libp2p gossip/sync/discovery wired to node |
| Staking & PoS | **DONE** | Validator bonding, graduated slash, PoS block production, `--validate` |
| Security hardening | **DONE** | Eclipse protection, subnet diversity, DoS limits, memory challenge |
| Testnet | **READY** | Code complete; deploy and run public testnet |
| Mainnet | **PLANNED** | Genesis ceremony and launch |

---

## License

MIT