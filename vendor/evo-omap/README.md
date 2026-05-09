# EVO-OMAP — Evolving Oriented Memory-hard Algorithm for Proof-of-work

## Overview

EVO-OMAP (Evolving Oriented Memory-hard Algorithm for Proof-of-work) is a memory-hard proof-of-work algorithm designed to achieve equitable hashrate distribution across CPU, GPU, and ASIC hardware. Unlike hash-based PoWs (SHA-256, Scrypt), EVO-OMAP is **execution-based** — miners must execute a deterministic program that reads and writes a large memory buffer, forcing high DRAM bandwidth requirements on all mining hardware.

## Key Properties

| Property | Value | Implication |
|----------|-------|-------------|
| Memory Footprint | 256 MiB | Requires 256 MiB DRAM bandwidth |
| Memory Access | Read-write per step | Random read-write pattern saturates DRAM bandwidth |
| Branch Factor | 4-way | GPU warp efficiency reduced; CPUs with branch prediction excel |
| Execution Model | Superscalar | 16 instructions per step with data-dependent operands |
| State Size | 512 bits | Small enough for full in-register execution |
| Dataset Chaining | Sequential | Nodes form a chain; no parallel precomputation |

## Why EVO-OMAP?

### The Memory-Hardness Problem

Traditional hash-based PoWs (SHA-256, Scrypt, Argon2) compute a pure function — given the same input, they always produce the same output with no side effects. This allows ASICs to:

1. Compute the function in minimal silicon (no RAM required)
2. Pipeline the computation for high clock frequencies
3. Skip memory entirely, achieving 1000-10000x efficiency gains over CPUs

### EVO-OMAP's Solution

EVO-OMAP requires a **mutable 256 MiB dataset** that changes with every nonce attempt. The algorithm:

1. Reads two nodes from the dataset each step
2. Executes 64 instructions using data from those nodes as operands
3. Applies a 4-way data-dependent branch
4. Writes one node back to the dataset

This creates **memory-read-write dependence** — the dataset must be physically present in memory and accessed in a pattern determined by intermediate computational results. An ASIC cannot skip this; it must include 256 MiB of fast memory and a memory controller.

### Mining Parallelism

Dataset generation is **sequential** (each node depends on the previous), but nonce search is **embarrassingly parallel**. The `mine_parallel()` function distributes nonce ranges across CPU cores using [rayon](https://github.com/rayon-rs/rayon), with a shared `Arc<Dataset>` for read-only access. Each thread maintains its own `CowDataset` (copy-on-write view) to track modifications without contention.

## Algorithm Specification

### Notation

- `H(x)` — Blake3-256 hash of `x`
- `XOF(seed, len)` — Blake3 XOF extending `seed` to `len` bytes
- `⊕` — XOR operation
- `←` — Assignment
- `%` — Modular arithmetic

### Dataset Generation

The dataset consists of `N = 256` nodes, each of size `S = 1,048,576` bytes (1 MiB).

```
Node[0] = XOF(DOMAIN_NODE ‖ epoch_seed ‖ 0, S)

For i = 1 to N-1:
    Node[i] = XOF(DOMAIN_NODE ‖ epoch_seed ‖ Node[i-1] ‖ i, S)
```

The default **epoch seed** is derived from block height:
```
epoch = floor(height / EPOCH_LENGTH)
epoch_seed = H(DOMAIN_EPOCH ‖ epoch)
```

Production chains can bind the epoch seed to chain history or chain identity by
passing seed material:
```
epoch_seed = H(prefixed(DOMAIN_EPOCH) ‖ epoch_u64_le ‖ len(seed_material)_u64_le ‖ seed_material)
```

For example, Opolys passes `MAINNET_CHAIN_ID || previous_block_hash` so future
datasets cannot be precomputed without knowing real chain history.

### State

The state `W` is 64 bytes (512 bits), interpreted as 8 × unsigned 64-bit integers:
```
W = [w₀, w₁, ..., w₇]
```

### State Initialization

The initial state is derived from the block header, height, and nonce. The header is first committed with Blake3-256 (preventing truncation attacks), then the mining seed is derived. All domain separators are length-prefixed (`len_u8 ‖ domain_bytes`) to prevent cross-domain collisions:
```
header_commitment = H(header)
mining_seed = H(prefixed(DOMAIN_SEED) ‖ 32_u64_le ‖ header_commitment ‖ height_u64_le ‖ nonce_u64_le)
state = XOF(mining_seed, 64)
```

### Mining Seed

```
header_commitment = H(header)
mining_seed = H(prefixed(DOMAIN_SEED) ‖ 32_u64_le ‖ header_commitment ‖ height_u64_le ‖ nonce_u64_le)
```
where `prefixed(d) = len(d)_u8 ‖ d`.

### Program Execution

At each of `T = 4096` steps:

1. **Index Derivation**: Compute dataset indices from state:
   ```
   idx₁ = ((w₀ + step) × (w₄ + 1)) mod N
   idx₂ = (w₁ × (step + 1) + w₅) mod N
   idx_w = (w₂ ⊕ w₃ ⊕ step) mod N
   ```

2. **Program Generation**: Derive 16 instructions from state using golden-ratio mixing:
   ```
   For i = 0 to 15:
       word_idx = i mod 8
       selector = w[word_idx] + i × 0x9e3779b97f4a7c15  (wrapping, Knuth TAOCP)
       op  = selector & 0x07
       dst = (selector >> 3) & 0x07
       src = (selector >> 6) & 0x7F
   ```
   The golden-ratio constant (`2^64 / φ`) ensures that instructions sharing
   the same `word_idx` (e.g. i=0 and i=8) produce distinct selectors.

3. **Instruction Execution**: Each instruction accesses `node[src mod 128]`:
   ```
   ADD:   w[dst] = w[dst] + node1[src mod 128]
   SUB:   w[dst] = w[dst] - node2[src mod 128]
   MUL:   w[dst] = w[dst] × node1[src mod 128]
   XOR:   w[dst] = w[dst] ⊕ node2[src mod 128]
   ROTL:  w[dst] = w[dst] << (node1[src mod 128] mod 64)
   ROTR:  w[dst] = w[dst] >> (node2[src mod 128] mod 64)
   MULH:  w[dst] = high_64(w[dst] × node1[src mod 128])
   SWAP:  swap(w[a], w[b])
   ```

4. **Branch Application**: Select branch variant and mix. All 4 variants consume both nodes. The variant byte is included in the hash input before the node bytes, so all four produce distinct XOF outputs even though variants 0&2 share the same node order and variants 1&3 share the same node order:
   ```
   variant = w₀ mod 4
   input = prefixed(DOMAIN_BRANCH) ‖ step_u32_le ‖ variant_u8
   variants 0, 2: input ‖= state ‖ node1[0..32] ‖ node2[0..32]
   variants 1, 3: input ‖= state ‖ node2[0..32] ‖ node1[0..32]
   output = XOF(input, 64)
   for j = 0 to 7: wⱼ = wⱼ ⊕ output[j×8:(j+1)×8]
   ```

5. **Node Write**: Compute and store written node (128 KiB working set per node):
   ```
   write_data = node1[0..131072] ‖ node2[0..131072] ‖ state[0..32]
   Node[idx_w] = XOF(write_data, S)
   ```

6. **Commitment Update**:
   ```
   commitment = H(commitment ‖ step_u64_le ‖ state[0..32])
   ```

7. **Initial Commitment**: Before the first step, initialize:
   ```
   commitment = H(DOMAIN_COMMITMENT ‖ height_u64_le)
   ```

### Finalization

```
state_summary = H(W)
leaf[i] = H(prefixed(DOMAIN_MEMORY) ‖ "leaf" ‖ i_u64_le ‖ len(Node[i])_u64_le ‖ Node[i])
parent = H(prefixed(DOMAIN_MEMORY) ‖ "parent" ‖ left_hash ‖ right_hash)
memory_commitment = MerkleRoot(leaf[0], ..., leaf[N-1])
final_hash = SHA3-256(state_summary ‖ commitment_hash ‖ memory_commitment)
```

### Hash Architecture

EVO-OMAP uses two different hash functions with distinct roles:

| Role | Function | Crate | Operations |
|------|----------|-------|------------|
| All internal operations | Blake3-256 / Blake3-XOF | `blake3` | Epoch seed, mining seed, node generation, rolling commitment, branch mixing, memory commitment |
| Final PoW output only | SHA3-256 | `sha3` | The single output checked against the difficulty target |

The finalization step is:
```
final_hash = SHA3-256(state_summary ‖ commitment_hash ‖ memory_commitment)
```
where every input to SHA3-256 was computed with Blake3. Using SHA3-256 (Keccak sponge) for the final step provides cryptographic diversity: an attacker would need to break both constructions to forge a proof.

### Difficulty

Difficulty `D` requires the 32-byte SHA3-256 final hash to have **at least D consecutive leading zero bits**. Expected attempts = **2^D**.

> **Note:** EVO-OMAP is a standalone PoW library — it accepts `difficulty` as an input parameter and returns a hash. Block time targets are **not** configured in this library; they belong in the blockchain code that calls evo-omap and adjusts `difficulty` to hit its desired block interval. The timing estimates below are for documentation reference only.

Block times below are based on parallel hashrate since `--parallel` is recommended for mining.

| Difficulty | Expected attempts | Acceptance rate | Block time @ 1.41 H/s (Ryzen 7 7700, 16 threads) | Block time @ 0.72 H/s (M1, 8 threads) |
|-----------|------------------|-----------------|---------------------------------------------------|----------------------------------------|
| 1 | 2 | ~50% | ~1 s | ~3 s |
| 4 | 16 | ~6.25% | ~11 s | ~22 s |
| 5 | 32 | ~3.1% | ~23 s | ~44 s |
| 7 | 128 | ~0.78% | ~91 s | ~3 min |
| 8 | 256 | ~0.39% | ~3 min | ~6 min |
| 10 | 1,024 | ~0.098% | ~12 min | ~24 min |
| 16 | 65,536 | ~0.0015% | ~12.9 hr | ~25 hr |

**Recommended difficulty for ~120s blocks:**
- Solo miner, Ryzen 7 7700 (`--parallel`, 16 threads): **difficulty 7** (~91 s expected)
- Solo miner, Apple M1 (`--parallel`, 8 threads): **difficulty 6** (~89 s expected at 0.72 H/s)
- Multi-miner network: difficulty is set dynamically by the blockchain code based on observed block times — evo-omap accepts whatever value is passed in

The `difficulty` parameter your blockchain passes to `mine_parallel()` controls block time — evo-omap itself has no block-time concept.

> **Important:** Always set `max_nonce` well above 2^difficulty. If `max_nonce < 2^difficulty` no nonce will be found. For difficulty 10: expected ~1,024 attempts. The CLI warns you if `max_nonce` is too low.

> **Note:** Prior to commit `e37129f`, difficulty was computed as `hash_int < u64::MAX / difficulty` — comparing only the first 8 bytes of the hash as a u64. That formula did not correctly represent mining difficulty and made `verify()` accept nearly any nonce at low difficulty values. The leading-zero-bit check above is the correct implementation.

## Memory-Hardness Analysis

### Why 256 MiB?

The memory requirement serves two purposes:

1. **Force DRAM bandwidth**: 256 MiB of DRAM must be accessed with large random read-write patterns across 4096 steps, each consuming 128 KiB per node read. This saturates memory bandwidth and cannot be avoided with caching.

2. **Create memory bandwidth ceiling**: Each step reads two 1 MiB nodes and writes one, processing 128 KiB of node data through Blake3. At 4096 steps this creates a sustained ~1 GiB/hash DRAM workload that bounds throughput by memory bandwidth, not compute.

### Why Operand Access?

The instruction set accesses `node_word[src mod 128]` — the first 1 KiB of each node. This creates **locality** in memory access patterns, but the 128-word window is large enough to prevent perfect caching.

### Why Data-Dependent Branching?

The 4-way branch depends on `w₀ mod 4`, which is determined by prior computation. This means:

- An ASIC cannot pre-fetch instructions
- A GPU warp takes divergent paths for different nonces
- Branch prediction helps but cannot eliminate misprediction

### Why Dataset Chaining?

Each node depends on the previous node via XOF. This means:

- Node[i] cannot be computed until Node[i-1] is known
- Dataset generation is O(N) sequential work
- Parallel computation of dataset is impossible

## ASIC Resistance Analysis

| Factor | CPU | GPU | Potential ASIC |
|--------|-----|-----|---------------|
| Memory | DRAM (shared) | DRAM (shared) | DRAM (high bandwidth) |
| Memory Cost | Low | Low | Moderate |
| Memory Bandwidth | 50 GB/s | 500 GB/s | 1000 GB/s |
| Branch Handling | Branch predictor | 32-thread warp divergence | Custom branch unit |
| Area Efficiency | N/A | N/A | Memory controller dominates |
| Expected Advantage | 1x (baseline) | 0.5-2x | 2-5x (limited by DRAM bandwidth) |

The theoretical ASIC advantage is **bounded by memory**, not unbounded as with pure hash functions.

## Parameters

### Default Configuration

| Parameter | Value | Range | Description |
|-----------|-------|-------|-------------|
| `NODE_SIZE` | 1,048,576 | 64 KiB - 16 MiB | Bytes per node |
| `NUM_NODES` | 256 | 16 - 1024 | Dataset nodes |
| `NUM_STEPS` | 4,096 | 512 - 65536 | Execution steps |
| `PROGRAM_LENGTH` | 16 | 4 - 16 | Instructions per step |
| `WRITE_NODE_PREFIX` | 131,072 | Fixed | Bytes per node mixed into write step (128 KiB) |
| `OPERAND_WORDS` | 128 | Fixed | Words per node for operand access |
| `BRANCH_WAYS` | 4 | 2, 4, 8 | Branch variants |
| `EPOCH_LENGTH` | 1,024 | 128 - 8192 | Blocks per epoch |

### Light Verification

Light verification reconstructs nodes on-demand by walking the dataset node chain from node 0, caching previously-computed original nodes. Modified nodes (from write steps) are tracked separately in a modification log. This approach:

- Requires no pre-generated cache (saves 32 MiB)
- Trades computation for memory
- Must recompute nodes that the full verification would have modified

The `LightDataset` implementation caches original nodes lazily and tracks mutations. Original nodes are always reconstructed through the **unmodified** predecessor chain (via `get_original_chain_node()`), regardless of which nodes were written during hashing. This matches the behaviour of the full `Dataset`, where unmodified nodes retain their pre-generated values even after their predecessor is overwritten.

**Full and light verification are guaranteed to produce identical hashes for the same inputs.** (A bug where `LightDataset` used modified predecessors during reconstruction, causing hash divergence, was fixed in commit `e37129f`.)

## Reference Implementation

### Project Structure

```
evo-omap/
├── src/
│   ├── hash.rs           # blake3, sha3 primitives
│   ├── public_spec.rs    # Algorithm specification (protocol constants)
│   ├── constants.rs      # Implementation parameters
│   ├── evo_omap.rs       # Core algorithm + tests
│   └── main.rs           # CLI interface
├── Cargo.toml
└── README.md
```

### API

```rust
use evo_omap::{
    Hash, mine, mine_parallel, verify, verify_light,
    mine_parallel_with_epoch_length_and_seed_material,
    verify_light_with_epoch_length_and_seed_material,
    CowDataset, DatasetCache, HashBuffers, evo_omap_hash_with_buffers,
};

// Single-threaded mining
let nonce = mine(header, height, difficulty, max_attempts);

// Multi-threaded mining — pass num_threads=0 to auto-detect CPU cores
// (equivalent to passing rayon::current_num_threads())
let nonce = mine_parallel(header, height, difficulty, max_attempts, 0);

// Full verification (requires 256 MiB)
let valid = verify(header, height, nonce, difficulty);

// Light verification (on-demand node reconstruction, no cache)
let valid = verify_light(header, height, nonce, difficulty);

// Optimized hashing with pre-allocated buffers (for batch processing)
let mut cow = CowDataset::new(&dataset);
let mut buffers = HashBuffers::new();
let hash = evo_omap_hash_with_buffers(&mut cow, header, height, nonce, &mut buffers);

// Epoch-based dataset caching (avoids regenerating on epoch boundaries)
let mut cache = DatasetCache::new();
let dataset = cache.get_dataset(height);

// Chain-bound mining with custom epoch length and seed material.
let seed_material = [chain_id_bytes.as_slice(), parent_hash.as_slice()].concat();
let (nonce, attempts) = mine_parallel_with_epoch_length_and_seed_material(
    header,
    height,
    difficulty,
    max_attempts,
    0,
    960,
    &seed_material,
);
let valid = verify_light_with_epoch_length_and_seed_material(
    header,
    height,
    nonce.unwrap(),
    difficulty,
    960,
    &seed_material,
);
```

### CLI

```bash
# Single-threaded mining
cargo run -- mine <header_hex> <height> <difficulty> [max_nonce]

# Multi-threaded mining (auto-detects CPU cores)
cargo run -- mine <header_hex> <height> <difficulty> [max_nonce] --parallel

# Multi-threaded mining with specified thread count
cargo run -- mine <header_hex> <height> <difficulty> [max_nonce] --parallel 8

# Verify (full)
cargo run -- verify <header_hex> <height> <nonce> <difficulty>

# Light verify (on-demand reconstruction)
cargo run -- verify <header_hex> <height> <nonce> <difficulty> light

# Compute hash
cargo run -- hash <header_hex> <height> <nonce>

# Epoch seed
cargo run -- seed <height> [seed_material_hex]
```

## Integration Guide

### Minimal blockchain integration

```rust
use evo_omap::{mine_parallel, verify};

// Mining a new block (num_threads=0 auto-detects CPU cores)
fn mine_block(header: &[u8], height: u64, difficulty: u64) 
    -> Option<u64> {
    let max_attempts = 1 << (difficulty + 4); // 16x expected attempts
    let (nonce, _) = mine_parallel(
        header, height, difficulty, max_attempts, 0
    );
    nonce
}

// Validating an incoming block
fn validate_block(header: &[u8], height: u64, 
                  nonce: u64, difficulty: u64) -> bool {
    verify(header, height, nonce, difficulty)
}
```

### Difficulty adjustment
EVO-OMAP accepts difficulty as an input — your blockchain code is 
responsible for adjusting it. A simple adjustment algorithm:

```rust
fn adjust_difficulty(current_difficulty: u64, 
                     actual_block_time_secs: f64,
                     target_block_time_secs: f64) -> u64 {
    // If blocks are coming 2x too fast, increase difficulty by 1 bit
    // If blocks are coming 2x too slow, decrease difficulty by 1 bit
    if actual_block_time_secs < target_block_time_secs / 2.0 {
        current_difficulty + 1
    } else if actual_block_time_secs > target_block_time_secs * 2.0 {
        current_difficulty.saturating_sub(1)
    } else {
        current_difficulty
    }
}
```

### Epoch boundary and seed-material handling
The default dataset regenerates every 1024 blocks. Blockchain integrations can
choose a different epoch length and can pass seed material that binds the
dataset to chain history.

Use `DatasetCache` to avoid regenerating while both the epoch number and seed
material are unchanged:

```rust
let mut cache = DatasetCache::new();
// Default library epoch handling.
let dataset = cache.get_dataset(height);

// Chain-specific epoch handling.
let seed_material = [chain_id_bytes.as_slice(), parent_hash.as_slice()].concat();
let dataset = cache.get_dataset_with_epoch_length_and_seed_material(
    height,
    960,
    &seed_material,
);
```

### Recommended starting difficulty
Start at difficulty 4 (16 expected attempts) for a single-miner 
testnet. Increase as more miners join.

## Performance

Performance optimizations for production mining:

| Component | Optimization |
|-----------|-------------|
| Buffer allocation | `HashBuffers` pre-allocates reusable Vec buffers for branch input, write data, commitment updates |
| CowDataset reset | Only clears modified indices (typically <50 of 256), not full array scan |
| Parallel mining | `mine_parallel()` uses rayon to distribute nonce ranges across CPU cores |
| Dataset caching | `DatasetCache` reuses dataset within same epoch, regenerating only on epoch boundary |
| Shared dataset | `Arc<Dataset>` allows read-only sharing across threads without cloning |

**Measured performance (release build, post security-fix v0.1):**

| Platform | Mode | Per-hash | Hashrate | Parallel speedup |
|----------|------|----------|----------|-----------------|
| Windows Ryzen 7 7700 (16 cores) | Single thread | ~3.6 s | ~0.28 H/s | — |
| Windows Ryzen 7 7700 (16 cores) | `--parallel` 16 threads | — | ~1.41 H/s | ~5.0× |
| Mac M1 (8 cores) | Single thread | ~6.1 s | ~0.16 H/s | — |
| Mac M1 (8 cores) | `--parallel` 8 threads | — | ~0.72 H/s | ~4.5× |

The M1 single-thread hashrate dropped from ~0.19 H/s to ~0.16 H/s after the security fixes. This is **intentional** — `WRITE_NODE_PREFIX` was restored from 8 KiB to 128 KiB (Fix 2), forcing the full 128 KiB working set per node write and making memory-hardness meaningful again.

Dataset generation is ~500 ms – 8 s depending on memory bandwidth (paid once per epoch).

Run the built-in benchmark to measure on your hardware:
```bash
# 5 iterations of dataset gen, per-hash, light verify, and full verify
./target/release/evo-omap bench 00 0 1 5
```

**Production recommendations:**
- Use `mine_parallel()` (`--parallel` flag) for maximum hashrate
- Single-threaded `mine()` is for benchmarking and verification only
- Use `DatasetCache` to avoid regenerating dataset within same epoch
- Use `HashBuffers` for batch hash computation

## Comparison with Other PoWs

| Algorithm | Memory | Memory Type | ASIC Advantage | Notes |
|----------|--------|-------------|---------------|-------|
| SHA-256d | None | None | ~1000x | Pure computation |
| Scrypt | 128 KiB | DRAM | ~100x | Memory-hard but small |
| Argon2 | 256 MiB | DRAM | ~10x | Password hashing, not mining |
| Ethash | 8 GiB | DRAM | ~2x | Dataset grows over time |
| **EVO-OMAP** | **256 MiB** | **DRAM** | **2-5x** | **Mutable dataset** |

EVO-OMAP's advantage: the mutable dataset with random read-write dependence requires sustained high-bandwidth DRAM access that cannot be skipped, cached, or computed ahead of time.

## Security Considerations

### 1. Memory-Efficient Mining

An ASIC with dedicated high-bandwidth DRAM could potentially:
- Cache the entire dataset closer to compute
- Achieve higher memory bandwidth than commodity DRAM

However, 256 MiB of high-bandwidth DRAM requires significant board area and power budget, limiting the parallel instances an ASIC can run.

### 2. Light Client Trust

Light clients verify using on-demand node reconstruction (`LightDataset`). `verify()` and `verify_light()` are guaranteed to agree: both compute the same final SHA3-256 hash for any given `(header, height, nonce)` tuple. The memory commitment is a Merkle root over indexed node leaves, and the library exposes proof helpers for O(log n) node-membership proofs.

### 3. Big-Endian Platforms

The algorithm uses little-endian byte interpretation for arithmetic. Big-endian
platforms would produce different hashes, causing chain splits. This is
explicitly **not supported**: the crate fails to compile on non-little-endian
targets.

### 4. Security Fixes

**Security audit summary:** 20 areas reviewed — 17 passed outright, 3 findings identified, all 3 addressed.

**Commit `e37129f`** fixed three critical bugs affecting difficulty and verification logic (broken `u64::MAX / difficulty` target, `verify()` accepting arbitrary nonces, `verify_light()` hash divergence from full verify).

**Commit `249cad7`** applied 12 further fixes from the full audit:

| Finding | Fix | Impact |
|---------|-----|--------|
| 3.1 | Header truncation → `blake3_256(header)` commitment | Prevents header extension attacks |
| 2.1 | `WRITE_NODE_PREFIX` 8 KiB → 128 KiB | Restores 128 KiB memory working set per step |
| 2.3 | Write index mixes step counter | Prevents write-index prediction |
| 3.2 | `mine_parallel` atomics: Release/Acquire + `compare_exchange` | Prevents data races on nonce result |
| 3.5 | `DatasetCache` sentinel `u64::MAX` → `Option<u64>` | Prevents false cache hit at astronomical block heights matching u64::MAX sentinel |
| 4.2 | `LightDataset::set_node` length assertion | Catches malformed nodes at insert |
| 2.4 | `PROGRAM_LENGTH` 8 → 16 with golden-ratio mixing | Increases computation per step; position-dependent selectors prevent repeated instructions |
| 2.5 | Branch variants now each consume both nodes exactly once | Restores branch symmetry |
| 1.2 | Domain separators length-prefixed at all 13 call sites | Prevents cross-domain collisions |
| 4.5 | Remove dead `Cache`/`generate_cache` code | Removes unused attack surface |
| 3.4 | `LightDataset::reset()` + call in `verify_light` | Prevents state leakage between verifications |
| 2.2 | Read indices mix `words[4]`/`words[5]` | Improves index unpredictability |

## Known Limitations

### Chain-Bound Seed Material Is Integration Responsibility
The default API remains deterministic from `(height, EPOCH_LENGTH)` for
standalone compatibility and reproducible tests. Production blockchains should
use the `*_with_epoch_length_and_seed_material` APIs and pass chain-specific
material such as `chain_id || parent_hash`. EVO-OMAP cannot choose that material
itself because it is chain-state dependent.

### Light Client Proof Scope
EVO-OMAP exposes Merkle membership proofs for committed memory nodes. Full PoW
verification still requires executing the algorithm or using `verify_light()`;
the Merkle proof helpers are for node-membership proofs against a committed
memory root, not a standalone replacement for PoW execution.

### Network Security at Launch
At current hashrates (~0.16-1.48 H/s per machine), a single 
additional machine can match a small honest network. Real economic 
security requires a sufficiently large and distributed mining 
network. Difficulty should be set conservatively at launch.

### Big-Endian Platforms
Not supported. The crate fails compilation on non-little-endian targets because
the algorithm uses little-endian byte interpretation for consensus arithmetic.

## Test Vectors

Test vectors are computed dynamically in the test module (`evo_omap.rs`). Run tests with `cargo test` to verify correctness against known-answer tests.

See `evo_omap.rs` test module for full test suite including:
- Dataset generation determinism
- Instruction execution correctness
- Program generation validation
- Domain separator collision tests
- Edge case tests (overflow, max values)

## Academic Background

EVO-OMAP builds on principles from:

1. **Memory-hard functions**: Percival & Colvin (Scrypt, 2009)
2. **Data-dependent addressing**: Boneh et al. (Balloon Hashing, 2015)
3. **DAG-based mining**: Ethash (2014)
4. **Execution-based PoW**: RandomJS (2018), RandomX (2019)

Key innovations:
- Mutable dataset with read-write dependence
- All instructions access memory (not just mining seed)
- 4-way branching with variable node data mixing

## Changelog

### v0.3.0 (current)
- Replaced the linear memory commitment with a Merkle-root commitment over
  indexed node leaves
- Added `MemoryMerkleProof` / `MemoryMerkleSibling` plus proof generation and
  verification helpers
- This is a breaking PoW-output change and should be adopted only before chain
  launch or with an explicit network upgrade

### v0.2.1
- Added custom epoch length APIs for blockchain integrations that use epoch
  lengths other than EVO-OMAP's 1024-block default
- Added seed-material APIs so production chains can bind datasets to chain
  identity and parent block hash, preventing durable future-dataset
  precomputation
- `DatasetCache` now keys cache validity by both epoch and seed material
- Added little-endian compile guard for consensus safety on unsupported targets
- CLI `seed` accepts optional seed material hex
- Benchmark validation loop now uses the leading-zero-bit difficulty rule

### v0.2.0
- **mine_parallel(0) auto-detects threads** (commit `3218cf7`): passing
  `num_threads=0` now maps to `rayon::current_num_threads()` instead of
  silently returning `None` with no work attempted
- **Program generation fix** (commit `a85a9b6`): fixed instruction
  repetition bug in `generate_program`; instructions now use
  golden-ratio mixing (`0x9e3779b97f4a7c15`) so slots sharing the same
  `word_idx` produce distinct selectors
- **Security audit complete**: 20 areas reviewed, 17 passed, 3 findings
  identified — all 3 addressed (see Security Fixes table)
- README synced with final benchmark numbers (Ryzen 1.41 H/s,
  M1 0.72 H/s parallel) and audit results
- `PROGRAM_LENGTH` corrected to 16 throughout documentation
- Removed `authors` field from Cargo.toml
- Targeted consensus, seed-material, and platform checks passing

### v0.1.0
- Initial implementation of EVO-OMAP algorithm
- Critical security fix (commit `e37129f`): broken difficulty target,
  `verify()` accepting arbitrary nonces, `verify_light()` hash divergence
- 12 further security fixes from full audit (see Security Fixes table)
- Performance optimizations: +27% hashrate on M1, +115% on Windows
  Ryzen via node clone elimination and operand array truncation
- Parallel mining via atomic work-stealing nonce counter
- 87 tests passing, 0 failures

### API Stability
The API is not yet stable. Breaking changes may occur before v1.0.
Pin to a specific commit hash if building production systems on top
of this library.

## Contributing

### Running tests
cargo test --release         # full suite (~12 minutes)
cargo test --release -- --ignored   # should return 0 ignored

### Running benchmarks
```bash
./target/release/evo-omap mine 00 0 4 10000 --parallel
```

### Submitting changes
- Run `cargo check` and the focused consensus tests before submitting a PR
- Protocol changes (constants, hash construction, domain separators)
  are breaking changes and require a version bump
- Performance changes must include before/after hashrate numbers

## License

MIT OR Apache-2.0
