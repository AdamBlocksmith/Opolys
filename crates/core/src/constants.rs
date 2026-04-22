pub const CURRENCY_NAME: &str = "Opolys";
pub const CURRENCY_TICKER: &str = "OPL";
pub const CURRENCY_SMALLEST_UNIT: &str = "Fleck";
pub const FLECKS_PER_OPL: u64 = 10_000_000;

pub const SHARDS_PER_OPL: u64 = 1_000;
pub const SPARKS_PER_OPL: u64 = 1_000_000;
pub const DECIMAL_PLACES: u32 = 7;

pub const BASE_REWARD: u64 = 555_555 * FLECKS_PER_OPL;
pub const MIN_DIFFICULTY: u64 = 1;
pub const RETARGET_EPOCH: u64 = 1_000;
pub const POS_FINALITY_BLOCKS: u64 = 3;
pub const BLOCK_TARGET_TIME_SECS: u64 = 120;
pub const MIN_BOND_STAKE: u64 = 100 * FLECKS_PER_OPL;
pub const BLOCK_CAPACITY_RATE: u64 = 10_000;

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
        assert_eq!(SHARDS_PER_OPL, 1_000);
        assert_eq!(SPARKS_PER_OPL, 1_000_000);
        assert_eq!(FLECKS_PER_OPL, 10_000_000);
        assert_eq!(DECIMAL_PLACES, 7);
    }

    #[test]
    fn derived_constants() {
        assert_eq!(block_max_capacity_bytes(), 1_200_000);
    }

    #[test]
    fn base_reward_in_opl() {
        let reward_opl = BASE_REWARD as f64 / FLECKS_PER_OPL as f64;
        assert!((reward_opl - 555_555.0).abs() < 0.001);
    }

    #[test]
    fn min_bond_stake_in_opl() {
        let bond_opl = MIN_BOND_STAKE as f64 / FLECKS_PER_OPL as f64;
        assert!((bond_opl - 100.0).abs() < 0.001);
    }
}