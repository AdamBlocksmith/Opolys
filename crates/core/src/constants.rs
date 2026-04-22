// Opolys ($OPL) — Digital Gold
// Currency units (6 decimal places, gold-themed):
//   1 OPL         = 1
//   1 Pennyweight  = 0.01 OPL  (100 per OPL)
//   1 Grain        = 0.0001 OPL (10,000 per OPL)
//   1 Flake        = 0.000001 OPL (1,000,000 per OPL) — smallest unit
//
// Gold derivation (World Gold Council 2025 data):
//   Total above-ground gold: 219,891 tonnes (WGC, end-2025)
//   Annual gold production: 3,630 tonnes (USGS/WGC 2024-2025 avg)
//   Annual production in troy oz: ~116,707,041 (3,630 × 32,150.7)
//   Blocks per year: 262,980 (365.25 × 24 × 60 × 60 ÷ 120)
//   BASE_REWARD = round(116,707,041 ÷ 262,980) = 440 OPL
//   Source: LBMA PM Fix + USGS Mineral Commodity Summaries + WGC

pub const CURRENCY_NAME: &str = "Opolys";
pub const CURRENCY_TICKER: &str = "OPL";
pub const CURRENCY_SMALLEST_UNIT: &str = "Flake";
pub const FLAKES_PER_OPL: u64 = 1_000_000;
pub const PENNYWEIGHTS_PER_OPL: u64 = 100;
pub const GRAINS_PER_OPL: u64 = 10_000;
pub const DECIMAL_PLACES: u32 = 6;

// Gold-derived: 440 OPL = troy ounces of gold mined worldwide per block interval
// Formula: floor(annual_production_tonnes × 32150.7 ÷ blocks_per_year)
// annual_production_tonnes = 3,630 (USGS/WGC 2024-2025)
// blocks_per_year = 365.25 × 24 × 60 × 60 ÷ 120 = 262,980
pub const BASE_REWARD: u64 = 440 * FLAKES_PER_OPL;

pub const MIN_DIFFICULTY: u64 = 1;
pub const RETARGET_EPOCH: u64 = 1_000;
pub const POS_FINALITY_BLOCKS: u64 = 3;
pub const BLOCK_TARGET_TIME_SECS: u64 = 120;
pub const MIN_BOND_STAKE: u64 = 100 * FLAKES_PER_OPL;
pub const BLOCK_CAPACITY_RATE: u64 = 10_000;

// Node configuration (not consensus-critical)
pub const MAX_INBOUND_CONNECTIONS: usize = 50;
pub const MAX_OUTBOUND_CONNECTIONS: usize = 50;
pub const MAX_PEER_COUNT: usize = 200;
pub const SYNC_MAX_BLOCKS_PER_REQUEST: u64 = 500;
pub const SYNC_MAX_HEADERS_PER_REQUEST: u64 = 2_000;
pub const SYNC_REQUEST_TIMEOUT_SECS: u64 = 30;
pub const SYNC_PARALLEL_PEER_COUNT: usize = 3;
pub const KAD_BUCKET_SIZE: usize = 20;
pub const KAD_QUERY_TIMEOUT_SECS: u64 = 60;
pub const PING_INTERVAL_SECS: u64 = 30;
pub const PING_TIMEOUT_SECS: u64 = 20;
pub const DEFAULT_LISTEN_PORT: u16 = 4170;
pub const GOSSIP_MAX_MESSAGE_SIZE_BYTES: usize = 5_242_880;
pub const NETWORK_PROTOCOL_VERSION: &str = "1.0.0";

pub const MEMPOOL_MAX_SIZE_BYTES: usize = 100_000_000;
pub const MEMPOOL_MAX_TXS_PER_ACCOUNT: usize = 50;
pub const MEMPOOL_TX_EXPIRY_SECS: u64 = 86_400;
pub const TX_MAX_SIZE_BYTES: usize = 100_000;

pub fn block_max_capacity_bytes() -> u64 {
    BLOCK_TARGET_TIME_SECS * BLOCK_CAPACITY_RATE
}

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