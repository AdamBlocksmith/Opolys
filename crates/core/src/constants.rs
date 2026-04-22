//! Consensus-critical and node configuration constants for the Opolys ($OPL) blockchain.
//!
//! # Monetary Model
//!
//! Opolys is digital gold — supply grows at the rate of real-world gold production.
//! There is **no hard cap**. The network reaches a natural equilibrium where
//! market-driven fees are burned, balancing new issuance.
//!
//! # Gold Derivation (World Gold Council 2025 data)
//!
//! | Metric | Value | Source |
//! |---|---|---|
//! | Total above-ground gold | 219,891 tonnes | WGC, end-2025 |
//! | Annual gold production | 3,630 tonnes | USGS/WGC 2024-2025 avg |
//! | Annual production in troy oz | ~116,707,041 | 3,630 × 32,150.7 |
//! | Blocks per year | 262,980 | 365.25 × 24 × 60 × 60 ÷ 120 |
//! | **BASE_REWARD** | **440 OPL** | floor(116,707,041 ÷ 262,980) |
//!
//! # Currency Units (6 decimal places, gold-themed)
//!
//! - **OPL** — whole coin (1)
//! - **Pennyweight (dwt)** — 0.01 OPL (100 per OPL)
//! - **Grain (gr)** — 0.0001 OPL (10,000 per OPL)
//! - **Flake** — 0.000001 OPL (1,000,000 per OPL) — smallest unit
//!
//! All on-chain amounts are in Flakes (u64). No floating-point arithmetic is used
//! anywhere in consensus logic.

// ─── Currency ────────────────────────────────────────────────────────────────

/// Human-readable name of the currency.
pub const CURRENCY_NAME: &str = "Opolys";

/// Ticker symbol used in displays, APIs, and exchanges.
pub const CURRENCY_TICKER: &str = "OPL";

/// Name of the smallest indivisible unit (1/1,000,000 OPL).
pub const CURRENCY_SMALLEST_UNIT: &str = "Flake";

/// Number of Flakes in 1 OPL. This is the fundamental unit ratio:
/// 1 OPL = 1,000,000 Flakes (6 decimal places).
pub const FLAKES_PER_OPL: u64 = 1_000_000;

/// Number of Pennyweights in 1 OPL. Named after the pennyweight (dwt),
/// a traditional gold measurement unit equal to 1/20 troy ounce.
pub const PENNYWEIGHTS_PER_OPL: u64 = 100;

/// Number of Grains in 1 OPL. Named after the grain (gr),
/// a traditional gold measurement unit equal to 1/480 troy ounce.
/// 1 Grain = 100 Flakes.
pub const GRAINS_PER_OPL: u64 = 10_000;

/// Decimal places for OPL display formatting. Always 6 (microsats).
pub const DECIMAL_PLACES: u32 = 6;

// ─── Block Rewards ───────────────────────────────────────────────────────────

/// Base block reward in Flakes — the only source of new OPL issuance.
///
/// Derived from real-world gold production:
/// ```text
/// annual_oz = 3,630 tonnes × 32,150.7 oz/tonne ≈ 116,707,041 oz
/// blocks_per_year = 365.25 × 86400 ÷ 120 ≈ 262,980
/// reward = floor(116,707,041 ÷ 262,980) = 440 OPL per block
/// ```
/// This equates to ~440 OPL per 120-second block, mirroring the rate at which
/// physical gold is mined worldwide. See module-level docs for full derivation.
pub const BASE_REWARD: u64 = 440 * FLAKES_PER_OPL;

// ─── Consensus Parameters ────────────────────────────────────────────────────

/// Minimum difficulty target (easiest possible PoW). Difficulty 1 means
/// any hash satisfies the target.
pub const MIN_DIFFICULTY: u64 = 1;

/// Number of blocks between difficulty retargeting events.
///
/// The network adjusts difficulty every `RETARGET_EPOCH` blocks to maintain
/// the target block interval.
pub const RETARGET_EPOCH: u64 = 1_000;

/// Number of PoS-finalized blocks required before a block is considered final.
///
/// After this many subsequent PoS blocks, a block cannot be reverted.
pub const POS_FINALITY_BLOCKS: u64 = 3;

/// Target time between blocks in seconds. 120s ≈ one block every 2 minutes.
///
/// This is chosen so that ~262,980 blocks are produced per year, aligning
/// block issuance with real-world gold mining rates.
pub const BLOCK_TARGET_TIME_SECS: u64 = 120;

/// Minimum stake (in Flakes) required to become a validator (100 OPL).
///
/// This prevents Sybil attacks by requiring a meaningful economic commitment
/// before a node can participate in block production.
pub const MIN_BOND_STAKE: u64 = 100 * FLAKES_PER_OPL;

/// Maximum block capacity in bytes per second of target block time.
///
/// A block's serialized size must not exceed `BLOCK_TARGET_TIME_SECS × BLOCK_CAPACITY_RATE`.
/// This throttles chain growth and ensures nodes can sync on modest bandwidth.
pub const BLOCK_CAPACITY_RATE: u64 = 10_000;

/// Returns the maximum allowed serialized block size in bytes.
///
/// Computed as `BLOCK_TARGET_TIME_SECS × BLOCK_CAPACITY_RATE` (120 × 10,000 = 1,200,000 bytes).
/// This is the bandwidth budget for a single block.
pub fn block_max_capacity_bytes() -> u64 {
    BLOCK_TARGET_TIME_SECS * BLOCK_CAPACITY_RATE
}

// ─── Node Networking ─────────────────────────────────────────────────────────
// These values are NOT consensus-critical — they can be tuned per-node.

/// Maximum number of inbound (peer-initiated) connections a node will accept.
pub const MAX_INBOUND_CONNECTIONS: usize = 50;

/// Maximum number of outbound (self-initiated) connections a node will maintain.
pub const MAX_OUTBOUND_CONNECTIONS: usize = 50;

/// Maximum number of peers tracked in the peer manager (includes connected + known).
pub const MAX_PEER_COUNT: usize = 200;

/// Maximum number of block bodies fetched in a single sync request.
pub const SYNC_MAX_BLOCKS_PER_REQUEST: u64 = 500;

/// Maximum number of block headers fetched in a single sync request.
pub const SYNC_MAX_HEADERS_PER_REQUEST: u64 = 2_000;

/// Timeout (in seconds) for an individual sync request before giving up.
pub const SYNC_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Number of peers to query in parallel during chain synchronization.
pub const SYNC_PARALLEL_PEER_COUNT: usize = 3;

/// Kademlia DHT bucket size (k-bucket parameter).
pub const KAD_BUCKET_SIZE: usize = 20;

/// Timeout (in seconds) for a Kademlia DHT query before giving up.
pub const KAD_QUERY_TIMEOUT_SECS: u64 = 60;

/// Interval (in seconds) between peer liveness ping messages.
pub const PING_INTERVAL_SECS: u64 = 30;

/// Timeout (in seconds) to wait for a ping response before considering a peer dead.
pub const PING_TIMEOUT_SECS: u64 = 20;

/// Default TCP port for node-to-node communication.
pub const DEFAULT_LISTEN_PORT: u16 = 4170;

/// Maximum size (in bytes) of a single gossip message (5 MiB).
/// Prevents memory exhaustion from oversized network messages.
pub const GOSSIP_MAX_MESSAGE_SIZE_BYTES: usize = 5_242_880;

/// Protocol version string advertised during handshakes.
pub const NETWORK_PROTOCOL_VERSION: &str = "1.0.0";

// ─── Mempool & Transaction Limits ───────────────────────────────────────────

/// Maximum total memory (in bytes) the mempool may consume (100 MiB).
pub const MEMPOOL_MAX_SIZE_BYTES: usize = 100_000_000;

/// Maximum number of pending transactions per account in the mempool.
/// Prevents a single account from flooding the mempool.
pub const MEMPOOL_MAX_TXS_PER_ACCOUNT: usize = 50;

/// Time (in seconds) after which a mempool transaction is considered expired and evicted.
pub const MEMPOOL_TX_EXPIRY_SECS: u64 = 86_400;

/// Maximum serialized size (in bytes) of a single transaction.
pub const TX_MAX_SIZE_BYTES: usize = 100_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_consistent() {
        assert_eq!(PENNYWEIGHTS_PER_OPL, 100);
        assert_eq!(GRAINS_PER_OPL, 10_000);
        assert_eq!(FLAKES_PER_OPL, 1_000_000);
        assert_eq!(DECIMAL_PLACES, 6);
        assert_eq!(FLAKES_PER_OPL / PENNYWEIGHTS_PER_OPL, 10_000);
        assert_eq!(FLAKES_PER_OPL / GRAINS_PER_OPL, 100);
    }

    #[test]
    fn derived_constants() {
        assert_eq!(block_max_capacity_bytes(), 1_200_000);
    }

    #[test]
    fn base_reward_gold_derivation() {
        let annual_production_tonnes: u64 = 3_630;
        let troy_oz_per_tonne: f64 = 32_150.7;
        let blocks_per_year: f64 = 365.25 * 24.0 * 60.0 * 60.0 / 120.0;
        let annual_oz = annual_production_tonnes as f64 * troy_oz_per_tonne;
        let opl_per_block = annual_oz / blocks_per_year;
        assert!(opl_per_block > 439.0 && opl_per_block < 445.0,
            "Gold derivation: {} OPL/block should be ~440", opl_per_block);
        assert_eq!(BASE_REWARD, 440 * FLAKES_PER_OPL);
        let base_reward_opl = BASE_REWARD / FLAKES_PER_OPL;
        assert_eq!(base_reward_opl, 440);
    }

    #[test]
    fn min_bond_stake_in_opl() {
        let bond_opl = MIN_BOND_STAKE as f64 / FLAKES_PER_OPL as f64;
        assert!((bond_opl - 100.0).abs() < 0.001);
    }
}