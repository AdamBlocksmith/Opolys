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
//! | Blocks per year | 374,267 | 365.25 × 86,400 ÷ 84.375 |
//! | **BASE_REWARD** | **312 OPL** | floor(116,707,041 ÷ 374,256) |
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

/// Base block reward in Flakes — the only source of new OPL issuance.
///
/// Derived from real-world gold production data:
/// ```text
/// annual_oz = 3,630 tonnes × 32,150.7 oz/tonne ≈ 116,707,041 oz
/// blocks_per_year = 365.25 epochs × 1,024 blocks = 374,256
/// reward = floor(116,707,041 ÷ 374,256) = 312 OPL per block
/// ```
/// With 84,375-second blocks (exactly 1,024 blocks per 24 hours),
/// each block earns a base of 312 OPL. Vein yield and difficulty adjust
/// the actual reward upward or downward from this base.
pub const BASE_REWARD: u64 = 312 * FLAKES_PER_OPL;

// ─── Consensus Parameters ────────────────────────────────────────────────────

/// Minimum difficulty target (easiest possible PoW). Difficulty 1 means
/// any hash satisfies the target. This is a mathematical floor, not an
/// arbitrary cap — the natural logarithm in vein yield is undefined for
/// difficulty 0.
pub const MIN_DIFFICULTY: u64 = 1;

/// Starting difficulty for the genesis block.
/// At 1.48 H/s (Ryzen 7 7700 parallel) this gives ~91 second blocks,
/// close to the 84 second target. The first retarget at block 1,024
/// (~26 hours) will correct any deviation automatically.
/// Must be >= MIN_DIFFICULTY and calibrated to typical launch hardware.
pub const GENESIS_DIFFICULTY: u64 = 7;

/// Unified epoch length for both EVO-OMAP dataset regeneration and
/// difficulty retargeting. Every 1,024 blocks:
/// - EVO-OMAP generates a new dataset from a fresh epoch seed
/// - Difficulty is retargeted based on observed block times
/// - Unbonding entries mature (1,024 blocks = one epoch delay)
///
/// Replaces the previous separate RETARGET_EPOCH and EVO_OMAP_EPOCH_BLOCKS
/// constants with a single unified EPOCH.
pub const EPOCH: u64 = 1_024;

/// Number of blocks a validator must wait before unbonded stake is returned.
/// Equal to EPOCH (1,024 blocks ≈ 34 hours at 120s/block). Unbonding stake
/// still earns rewards during this delay.
pub const UNBONDING_DELAY_BLOCKS: u64 = EPOCH;

/// Minimum transaction fee in Flakes. 1 Flake is the atomic unit — this
/// establishes the floor for the market-driven fee model. The actual
/// suggested fee is computed as an EMA of the previous block's transaction
/// fees, but can never fall below MIN_FEE.
/// Minimum transaction fee in Flakes. 1 Flake = the atomic unit.
pub const MIN_FEE: u64 = 1;

/// Number of PoS-finalized blocks required before a block is considered final.
///
/// After this many subsequent PoS blocks, a block cannot be reverted.
pub const POS_FINALITY_BLOCKS: u64 = 3;

/// Target time between blocks in milliseconds.
/// 84,375 ms = 84.375 seconds per block.
///
/// Chosen so that exactly 1,024 blocks (one epoch) takes 24 hours:
/// 1,024 × 84,375 ms = 86,400,000 ms = 86,400 seconds = 24 hours.
///
/// This yields ~374,267 blocks per year, aligning block issuance with
/// real-world gold mining rates. BASE_REWARD (312 OPL) per block produces
/// an annual emission of ~312 × 374,256 ≈ 116.7 million OPL, closely
/// tracks the ~3,630 tonnes of annual gold production.
pub const BLOCK_TARGET_TIME_MS: u64 = 84_375;

/// Target time between blocks in seconds, rounded for convenience.
/// Use BLOCK_TARGET_TIME_MS for precise calculations.
pub const BLOCK_TARGET_TIME_SECS: u64 = 84;

/// Minimum stake (in Flakes) required for a **new** bond entry (1 OPL).
///
/// The natural unit — no arbitrary cap. FIFO unbonding consumes the oldest
/// entries first, and residuals from split entries require no minimum.
/// Only brand-new bond entries enforce this 1 OPL floor.
pub const MIN_BOND_STAKE: u64 = FLAKES_PER_OPL;

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

/// Maximum size (in bytes) of a single gossip message (5 MiB).
/// Prevents memory exhaustion from oversized network messages.
pub const GOSSIP_MAX_MESSAGE_SIZE_BYTES: usize = 5_242_880;

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

/// Maximum serialized size (in bytes) of a single block including all transactions.
/// 10 MiB allows ~100 full-size transactions or many small ones.
pub const MAX_BLOCK_SIZE_BYTES: usize = 10_485_760;

/// Maximum size (in bytes) of the `data` field in a transaction.
/// 1 KiB is enough for memo/attachment data without enabling block bloat.
pub const MAX_TX_DATA_SIZE_BYTES: usize = 1_024;

/// Maximum clock skew (in seconds) between a block's timestamp and the
/// local node's clock. Blocks with timestamps more than this many seconds
/// in the future are rejected.
pub const MAX_FUTURE_BLOCK_TIME_SECS: u64 = 300; // 5 minutes

// ─── Chain Identity ──────────────────────────────────────────────────────────
// Chain ID is included in transaction signing and ID computation to prevent
// cross-chain replay attacks. A valid mainnet transaction cannot be replayed
// on testnet and vice versa.

/// Chain ID for the Opolys mainnet. Included in transaction signing and tx_id
/// hashing to prevent replay attacks across networks.
pub const MAINNET_CHAIN_ID: u64 = 1;

/// Chain ID for the Opolys testnet.
pub const TESTNET_CHAIN_ID: u64 = 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_consistent() {
        assert_eq!(FLAKES_PER_OPL, 1_000_000);
        assert_eq!(DECIMAL_PLACES, 6);
    }

    #[test]
    fn base_reward_is_312_opl() {
        assert_eq!(BASE_REWARD, 312 * FLAKES_PER_OPL);
        let base_reward_opl = BASE_REWARD / FLAKES_PER_OPL;
        assert_eq!(base_reward_opl, 312);
    }

    #[test]
    fn block_target_time_produces_24h_epochs() {
        // 1,024 blocks × 84,375 ms = 86,400,000 ms = 86,400 s = 24 hours exactly
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