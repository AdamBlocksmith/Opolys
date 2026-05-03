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
//! | Blocks per year | 350,640 | 365.25 × 86400 / 90 |
//! | **BASE_REWARD** | **312 OPL** | floor(116,707,041 ÷ 374,016) |
//!
//! # Currency Units (6 decimal places)
//!
//! - **OPL** — whole coin (1)
//! - **Flake** — 0.000001 OPL (1,000,000 per OPL) — the smallest and only sub-unit
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
///
/// There is only one sub-unit: the Flake. No Pennyweight or Grain.
/// Display formatting uses 6 decimal places: 1.000000 OPL = 1,000,000 Flakes.
pub const FLAKES_PER_OPL: u64 = 1_000_000;

/// Decimal places for OPL display formatting. Always 6 (flakes).
pub const DECIMAL_PLACES: u32 = 6;

// ─── Block Rewards ───────────────────────────────────────────────────────────

/// Default base block reward in Flakes, used by testnet and pre-ceremony chains.
///
/// The mainnet base reward is determined by the genesis ceremony from live gold
/// production data (USGS/WGC/LBMA) and stored in `ChainState.base_reward`. This
/// constant serves as a fallback when no ceremony value is set.
///
/// Default derivation (2024 USGS/WGC data):
/// ```text
/// annual_oz = 3,630 tonnes × 32,150.7 oz/tonne ≈ 116,707,041 oz
/// blocks_per_year = 365.25 × 86400 / 90 = 350,640
/// reward = floor(116,707,041 ÷ 350,640) = 332 OPL per block
/// ```
/// The genesis ceremony computes the actual value using a trimmed median of
/// production data from 7+ independent sources, signs an attestation, and writes
/// genesis_params.rs. See `scripts/genesis_ceremony/src/main.rs`.
pub const BASE_REWARD: u64 = 332 * FLAKES_PER_OPL;

// ─── Consensus Parameters ────────────────────────────────────────────────────

/// Minimum difficulty target (easiest possible PoW). Difficulty 1 means
/// any hash satisfies the target. This is a mathematical floor, not an
/// arbitrary cap — the natural logarithm in vein yield is undefined for
/// difficulty 0.
pub const MIN_DIFFICULTY: u64 = 1;

/// Starting difficulty for the genesis block.
/// At difficulty 7, 1.48 H/s parallel (Ryzen 7 7700):
/// 2^7 / 1.48 = 86.5s per block, target 90s (3.9% off).
/// First retarget at block 960 self-corrects automatically.
/// Must be >= MIN_DIFFICULTY and calibrated to typical launch hardware.
pub const GENESIS_DIFFICULTY: u64 = 7;

/// Unified epoch length for both EVO-OMAP dataset regeneration and
/// difficulty retargeting. Every 960 blocks:
/// - EVO-OMAP generates a new dataset from a fresh epoch seed
/// - Difficulty is retargeted based on observed block times
/// - Unbonding entries mature (960 blocks = one epoch delay)
///
/// 90,000 ms × 960 blocks = 86,400,000 ms = exactly 24 hours.
pub const EPOCH: u64 = 960;

/// Approximate number of blocks per year.
/// Derived from: 365.25 × 86400 / 90 = 350,640
/// Used for annual supply attrition calculations.
pub const BLOCKS_PER_YEAR: u64 = (365 * 86400 + 86400 / 4) / 90; // 350_640

/// Annual gold attrition rate in permille (1.5% = 15 permille).
/// Derived from USGS/WGC data: ~1.5% of above-ground gold is lost annually
/// through wear, loss, and industrial consumption.
/// Applied across three channels:
/// - Mine assay: reduced issuance at block reward time
/// - Stake decay: epoch-based bonded stake reduction
/// - Bond/unbond assay: entry/exit fees
/// Total target: ~1.5% annual supply attrition, matching physical gold.
pub const ANNUAL_ATTRITION_PERMILLE: u64 = 15;

/// Number of blocks a refiner must wait before unbonded stake is returned.
/// Equal to EPOCH (960 blocks = exactly 24 hours at 90s/block). Unbonding stake
/// still earns rewards during this delay.
pub const UNBONDING_DELAY_BLOCKS: u64 = EPOCH;

/// Minimum transaction fee in Flakes. 1 Flake is the atomic unit — this
/// establishes the floor for the market-driven fee model. The actual
/// suggested fee is computed as an EMA of the previous block's transaction
/// fees, but can never fall below MIN_FEE.
/// Minimum transaction fee in Flakes. 1 Flake = the atomic unit.
pub const MIN_FEE: u64 = 1;

/// Target time between blocks in milliseconds.
/// 90,000 ms = 90 seconds per block.
///
/// Chosen so that exactly 960 blocks (one epoch) takes 24 hours:
/// 960 × 90,000 ms = 86,400,000 ms = 86,400 seconds = 24 hours.
///
/// This yields 350,640 blocks per year (365.25 × 86400 / 90), aligning block
/// issuance with real-world gold mining rates.
pub const BLOCK_TARGET_TIME_MS: u64 = 90_000;

/// Target time between blocks in seconds.
/// Use BLOCK_TARGET_TIME_MS for precise calculations.
pub const BLOCK_TARGET_TIME_SECS: u64 = 90;

/// Minimum stake (in Flakes) required for a **new** bond entry (1 OPL).
///
/// The natural unit — no arbitrary cap. FIFO unbonding consumes the oldest
/// entries first, and residuals from split entries require no minimum.
/// Only brand-new bond entries enforce this 1 OPL floor.
pub const MIN_BOND_STAKE: u64 = FLAKES_PER_OPL;

/// Maximum number of simultaneously active refiners.
///
/// Prevents unbounded O(n) per-block computation and disk writes.
/// Refiners that bond when the cap is full wait in `Bonding` status
/// until a slot opens (via unbond or slash). No `RefinerBond` transaction
/// is ever rejected — all refiners are queued fairly.
///
/// Can be raised via protocol upgrade.
/// Future upgrade path: soft-cap by weight (top-N by stake × seniority).
pub const MAX_ACTIVE_REFINERS: usize = 5_000;

/// Block header version number. Incremented for protocol upgrades.
/// Version 1 is the initial protocol version with EVO-OMAP PoW and
/// ed25519 signatures.
pub const BLOCK_VERSION: u32 = 1;

/// Signature type constant for ed25519 signatures.
/// Currently the only supported type. Post-quantum signatures (Dilithium)
/// use reserved values (1+), but are not yet implemented.
pub const SIGNATURE_TYPE_ED25519: u8 = 0;

/// Extension type constant: no extension data in this block.
pub const EXTENSION_TYPE_NONE: u8 = 0;

/// Extension type constant: rollup data included in this block.
/// Reserved for future use — rollups can anchor data via extension_root.
pub const EXTENSION_TYPE_ROLLUP: u8 = 1;

// ─── Node Networking ─────────────────────────────────────────────────────────
// These values are NOT consensus-critical — they can be tuned per-node.

/// Network protocol version string advertised during P2P handshakes
/// and returned by the `opl_getNetworkVersion` RPC.
pub const NETWORK_PROTOCOL_VERSION: &str = "1.0.0";

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

/// Maximum size (in bytes) of a single gossip message.
/// Must equal MAX_BLOCK_SIZE_BYTES — nodes must be able to relay any valid block.
/// Prevents memory exhaustion from oversized network messages.
pub const GOSSIP_MAX_MESSAGE_SIZE_BYTES: usize = MAX_BLOCK_SIZE_BYTES;

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

/// Maximum number of transactions in a single block.
/// Prevents blocks from growing indefinitely. At ~120s block time, 10,000
/// transactions per block = ~83 tx/s, which is generous for a digital gold chain.
pub const MAX_TRANSACTIONS_PER_BLOCK: usize = 10_000;

/// Maximum number of refiner block attestations carried in one block.
/// Keeps signature verification bounded once attestation finality is enabled.
pub const MAX_ATTESTATIONS_PER_BLOCK: usize = 1_024;

/// Minimum active-refiner attestation weight required to finalize a refiner block.
///
/// Expressed in milli-units: 667 = 66.7%, the standard 2/3+ Byzantine
/// threshold. Refiner-block finality only advances from attestations included
/// on-chain in a later block; mined blocks are secured by EVO-OMAP PoW.
pub const FINALITY_CONFIDENCE_MILLI: u64 = 667;

/// Maximum serialized size (in bytes) of a single block including all transactions.
/// 10 MiB allows ~100 full-size transactions or many small ones.
pub const MAX_BLOCK_SIZE_BYTES: usize = 10_485_760;

/// Capacity ratio: how many blocks the mempool can hold.
/// Derived from MEMPOOL_MAX_SIZE_BYTES / MAX_BLOCK_SIZE_BYTES ≈ 9.54, rounded to 10.
/// Used by the two-state fee model: when congested, suggested_fee is multiplied
/// by this ratio, reflecting that your transaction must outcompete ~10 blocks
/// worth of pending data to be included in the next block.
pub const CAPACITY_RATIO: u64 = 10;

/// Congestion threshold: fraction of mempool that must be occupied before
/// entering rush mode. Derived from 1/CAPACITY_RATIO — when there's more than
/// one block's worth of pending transactions, block producers have surplus
/// choice and fees should rise.
/// Stored as permille for integer arithmetic: 1000/CAPACITY_RATIO = 100.
pub const CONGESTION_THRESHOLD_PERMILLE: u64 = 1000 / CAPACITY_RATIO;

/// Maximum size (in bytes) of the `data` field in a transaction.
/// 1 KiB is enough for memo/attachment data without enabling block bloat.
pub const MAX_TX_DATA_SIZE_BYTES: usize = 1_024;

/// Maximum clock skew (in seconds) between a block's timestamp and the
/// local node's clock. Blocks with timestamps more than this many seconds
/// in the future are rejected.
pub const MAX_FUTURE_BLOCK_TIME_SECS: u64 = 300; // 5 minutes

// ─── Chain Identity ──────────────────────────────────────────────────────────
// Chain ID is included in transaction signing and ID computation to prevent
// cross-chain replay attacks.

/// Chain ID for the Opolys mainnet. Included in transaction signing and tx_id
/// hashing to prevent replay attacks. There is only one Opolys network.
pub const MAINNET_CHAIN_ID: u64 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_consistent() {
        assert_eq!(FLAKES_PER_OPL, 1_000_000);
        assert_eq!(DECIMAL_PLACES, 6);
    }

    #[test]
    fn base_reward_is_332_opl() {
        assert_eq!(BASE_REWARD, 332 * FLAKES_PER_OPL);
        let base_reward_opl = BASE_REWARD / FLAKES_PER_OPL;
        assert_eq!(base_reward_opl, 332);
    }

    #[test]
    fn block_target_time_produces_24h_epochs() {
        // 960 blocks × 90,000 ms = 86,400,000 ms = 86,400 s = 24 hours exactly
        let epoch_ms = EPOCH * BLOCK_TARGET_TIME_MS;
        assert_eq!(epoch_ms, 86_400_000);
    }

    #[test]
    fn min_bond_stake_is_one_opl() {
        assert_eq!(MIN_BOND_STAKE, FLAKES_PER_OPL);
    }

    #[test]
    fn epoch_equals_unbonding_delay() {
        assert_eq!(UNBONDING_DELAY_BLOCKS, EPOCH);
    }

    #[test]
    fn min_fee_is_one_flake() {
        assert_eq!(MIN_FEE, 1);
    }

    #[test]
    fn block_version_is_one() {
        assert_eq!(BLOCK_VERSION, 1);
    }

    #[test]
    fn signature_type_ed25519_is_zero() {
        assert_eq!(SIGNATURE_TYPE_ED25519, 0);
    }
}
