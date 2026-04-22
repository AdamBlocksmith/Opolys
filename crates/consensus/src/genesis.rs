use opolys_core::{Block, BlockHeader, BlockHeight, Hash, MIN_DIFFICULTY, BASE_REWARD, NETWORK_PROTOCOL_VERSION, BLOCK_TARGET_TIME_SECS, MIN_BOND_STAKE, FLECKS_PER_OPL, RETARGET_EPOCH, POS_FINALITY_BLOCKS, BLOCK_CAPACITY_RATE, CURRENCY_NAME, CURRENCY_TICKER, CURRENCY_SMALLEST_UNIT};
use borsh::{BorshSerialize, BorshDeserialize};
use opolys_crypto::Blake3Hasher;

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct GenesisConfig {
    pub initial_difficulty: u64,
    pub protocol_version: String,
}

impl Default for GenesisConfig {
    fn default() -> Self {
        GenesisConfig {
            initial_difficulty: MIN_DIFFICULTY,
            protocol_version: NETWORK_PROTOCOL_VERSION.to_string(),
        }
    }
}

pub fn build_genesis_block(config: &GenesisConfig) -> Block {
    let mut state_hasher = Blake3Hasher::new();
    state_hasher.update(config.protocol_version.as_bytes());
    state_hasher.update(&CURRENCY_NAME.as_bytes());
    state_hasher.update(&CURRENCY_TICKER.as_bytes());
    state_hasher.update(&CURRENCY_SMALLEST_UNIT.as_bytes());
    state_hasher.update(&FLECKS_PER_OPL.to_be_bytes());
    state_hasher.update(&BASE_REWARD.to_be_bytes());
    state_hasher.update(&BLOCK_TARGET_TIME_SECS.to_be_bytes());
    state_hasher.update(&MIN_DIFFICULTY.to_be_bytes());
    state_hasher.update(&RETARGET_EPOCH.to_be_bytes());
    state_hasher.update(&POS_FINALITY_BLOCKS.to_be_bytes());
    state_hasher.update(&MIN_BOND_STAKE.to_be_bytes());
    state_hasher.update(&BLOCK_CAPACITY_RATE.to_be_bytes());
    let state_root = state_hasher.finalize();

    Block {
        header: BlockHeader {
            height: 0,
            previous_hash: Hash::zero(),
            state_root,
            transaction_root: Hash::zero(),
            timestamp: 0,
            difficulty: config.initial_difficulty,
            pow_proof: None,
            validator_signature: None,
        },
        transactions: vec![],
    }
}

pub fn validate_genesis_block(block: &Block) -> Result<(), String> {
    if block.header.height != 0 {
        return Err("Genesis block must have height 0".to_string());
    }
    if block.header.previous_hash != Hash::zero() {
        return Err("Genesis block must have zero previous hash".to_string());
    }
    if block.header.pow_proof.is_some() {
        return Err("Genesis block must not have PoW proof".to_string());
    }
    if !block.transactions.is_empty() {
        return Err("Genesis block must not have transactions".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_block_creation() {
        let config = GenesisConfig::default();
        let genesis = build_genesis_block(&config);
        assert_eq!(genesis.header.height, 0);
        assert_eq!(genesis.header.previous_hash, Hash::zero());
        assert!(genesis.header.pow_proof.is_none());
        assert!(genesis.transactions.is_empty());
    }

    #[test]
    fn genesis_block_deterministic() {
        let config = GenesisConfig::default();
        let g1 = build_genesis_block(&config);
        let g2 = build_genesis_block(&config);
        assert_eq!(g1.header.state_root, g2.header.state_root);
    }

    #[test]
    fn genesis_block_validation() {
        let config = GenesisConfig::default();
        let genesis = build_genesis_block(&config);
        assert!(validate_genesis_block(&genesis).is_ok());
    }
}