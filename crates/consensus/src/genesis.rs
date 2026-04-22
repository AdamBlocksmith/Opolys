use opolys_core::{Block, BlockHeader, BlockHeight, Hash, MIN_DIFFICULTY, BASE_REWARD, NETWORK_PROTOCOL_VERSION, BLOCK_TARGET_TIME_SECS, MIN_BOND_STAKE, FLAKES_PER_OPL, RETARGET_EPOCH, POS_FINALITY_BLOCKS, BLOCK_CAPACITY_RATE, CURRENCY_NAME, CURRENCY_TICKER, CURRENCY_SMALLEST_UNIT};
use borsh::{BorshSerialize, BorshDeserialize};
use opolys_crypto::Blake3Hasher;

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct GenesisAttestation {
    pub ceremony_timestamp: u64,
    pub gold_spot_price_usd_cents: u64,
    pub annual_production_tonnes: u64,
    pub total_above_ground_tonnes: u64,
    pub lbma_response_hash: [u8; 32],
    pub usgs_response_hash: [u8; 32],
    pub wgc_response_hash: [u8; 32],
    pub derivation_formula: String,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct GenesisConfig {
    pub initial_difficulty: u64,
    pub protocol_version: String,
    pub attestation: GenesisAttestation,
}

impl Default for GenesisConfig {
    fn default() -> Self {
        GenesisConfig {
            initial_difficulty: MIN_DIFFICULTY,
            protocol_version: NETWORK_PROTOCOL_VERSION.to_string(),
            attestation: GenesisAttestation {
                ceremony_timestamp: 0,
                gold_spot_price_usd_cents: 0,
                annual_production_tonnes: 3630,
                total_above_ground_tonnes: 219891,
                lbma_response_hash: [0u8; 32],
                usgs_response_hash: [0u8; 32],
                wgc_response_hash: [0u8; 32],
                derivation_formula: "floor(annual_production_tonnes * 32150.7 / 262980)".to_string(),
            },
        }
    }
}

pub fn build_genesis_block(config: &GenesisConfig) -> Block {
    let mut state_hasher = Blake3Hasher::new();
    state_hasher.update(config.protocol_version.as_bytes());
    state_hasher.update(CURRENCY_NAME.as_bytes());
    state_hasher.update(CURRENCY_TICKER.as_bytes());
    state_hasher.update(CURRENCY_SMALLEST_UNIT.as_bytes());
    state_hasher.update(&FLAKES_PER_OPL.to_be_bytes());
    state_hasher.update(&BASE_REWARD.to_be_bytes());
    state_hasher.update(&BLOCK_TARGET_TIME_SECS.to_be_bytes());
    state_hasher.update(&MIN_DIFFICULTY.to_be_bytes());
    state_hasher.update(&RETARGET_EPOCH.to_be_bytes());
    state_hasher.update(&POS_FINALITY_BLOCKS.to_be_bytes());
    state_hasher.update(&MIN_BOND_STAKE.to_be_bytes());
    state_hasher.update(&BLOCK_CAPACITY_RATE.to_be_bytes());
    state_hasher.update(&config.attestation.ceremony_timestamp.to_be_bytes());
    state_hasher.update(&config.attestation.annual_production_tonnes.to_be_bytes());
    state_hasher.update(&config.attestation.total_above_ground_tonnes.to_be_bytes());
    state_hasher.update(&config.attestation.lbma_response_hash);
    state_hasher.update(&config.attestation.usgs_response_hash);
    state_hasher.update(&config.attestation.wgc_response_hash);
    state_hasher.update(config.attestation.derivation_formula.as_bytes());
    let state_root = state_hasher.finalize();

    Block {
        header: BlockHeader {
            height: 0,
            previous_hash: Hash::zero(),
            state_root,
            transaction_root: Hash::zero(),
            timestamp: config.attestation.ceremony_timestamp,
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

    #[test]
    fn genesis_attestation_defaults() {
        let config = GenesisConfig::default();
        assert_eq!(config.attestation.annual_production_tonnes, 3630);
        assert_eq!(config.attestation.total_above_ground_tonnes, 219891);
        assert!(!config.attestation.derivation_formula.is_empty());
    }
}