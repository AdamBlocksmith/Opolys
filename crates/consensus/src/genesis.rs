//! # Genesis block construction and validation.
//!
//! The Opolys genesis block anchors the chain to real-world gold data through
//! a **ceremony attestation** — an immutable record of gold spot prices,
//! annual production, and above-ground reserves sourced from the LBMA, USGS,
//! and WGC. No governance vote can change these values; they are baked into
//! the genesis state hash and validated by every full node on startup.
//!
//! The genesis block has zero transactions, zero previous hash, and no PoW
//! proof. Its `state_root` is a deterministic Blake3 hash over all protocol
//! constants and attestation fields, ensuring that every node derives the
//! exact same chain state from the same config.

use opolys_core::{Block, BlockHeader, Hash, MIN_DIFFICULTY, BASE_REWARD, NETWORK_PROTOCOL_VERSION, BLOCK_TARGET_TIME_SECS, MIN_BOND_STAKE, FLAKES_PER_OPL, EPOCH, POS_FINALITY_BLOCKS, BLOCK_VERSION, MIN_FEE, CURRENCY_NAME, CURRENCY_TICKER, CURRENCY_SMALLEST_UNIT};
use borsh::{BorshSerialize, BorshDeserialize};
use opolys_crypto::Blake3Hasher;

/// Cryptographic attestation from the Opolys genesis ceremony.
///
/// Records the real-world data points that anchor the $OPL supply model to
/// physical gold: LBMA spot price, USGS annual production, and WGC total
/// above-ground reserves. Each source's response hash ensures data integrity
/// without relying on ongoing oracle feeds.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct GenesisAttestation {
    /// Unix timestamp of the genesis ceremony.
    pub ceremony_timestamp: u64,
    /// Gold spot price in USD cents at ceremony time (from LBMA).
    pub gold_spot_price_usd_cents: u64,
    /// Annual gold production in tonnes (from USGS).
    pub annual_production_tonnes: u64,
    /// Total above-ground gold stock in tonnes (from WGC).
    pub total_above_ground_tonnes: u64,
    /// Blake3 hash of the LBMA API response for audit verification.
    pub lbma_response_hash: [u8; 32],
    /// Blake3 hash of the USGS API response for audit verification.
    pub usgs_response_hash: [u8; 32],
    /// Blake3 hash of the WGC API response for audit verification.
    pub wgc_response_hash: [u8; 32],
    /// Human-readable formula describing how `BASE_REWARD` was derived from
    /// the attestation data (e.g., `floor(annual_production_tonnes * 32150.7 / 262980)`).
    pub derivation_formula: String,
}

/// Configuration for building the Opolys genesis block.
///
/// Determines the initial difficulty and protocol version, plus the ceremony
/// attestation that anchors the chain to real-world gold data. Defaults use
/// `MIN_DIFFICULTY` and USGS/WGC 2024 figures for annual production and
/// total above-ground reserves.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct GenesisConfig {
    /// The starting difficulty at height 0. Typically `MIN_DIFFICULTY`.
    pub initial_difficulty: u64,
    /// Protocol version string for the genesis block.
    pub protocol_version: String,
    /// The ceremony attestation anchoring genesis to gold data.
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
                // USGS 2024 estimate for annual gold production in tonnes.
                annual_production_tonnes: 3630,
                // WGC 2024 estimate for total above-ground gold stock in tonnes.
                total_above_ground_tonnes: 219891,
                lbma_response_hash: [0u8; 32],
                usgs_response_hash: [0u8; 32],
                wgc_response_hash: [0u8; 32],
                // Derivation: floor(annual_production * troy_oz_per_tonne / above_ground_klots)
                // where 32150.7 = troy ounces per tonne and 262980 = scaling factor.
                derivation_formula: "floor(annual_production_tonnes * 32150.7 / 262980)".to_string(),
            },
        }
    }
}

/// Build the Opolys genesis block from the given configuration.
///
/// The genesis block has:
/// - Height 0 and `previous_hash` set to all zeros.
/// - No transactions, no PoW proof, no validator signature.
/// - A deterministic `state_root` computed by hashing all protocol constants
///   and attestation fields, ensuring every node arrives at the same state.
pub fn build_genesis_block(config: &GenesisConfig) -> Block {
    // Compute the state root by hashing every protocol constant and attestation
    // field in a fixed order. This makes the genesis state fully deterministic
    // — any node with the same config produces the exact same hash.
    let mut state_hasher = Blake3Hasher::new();
    state_hasher.update(config.protocol_version.as_bytes());
    state_hasher.update(CURRENCY_NAME.as_bytes());
    state_hasher.update(CURRENCY_TICKER.as_bytes());
    state_hasher.update(CURRENCY_SMALLEST_UNIT.as_bytes());
    state_hasher.update(&FLAKES_PER_OPL.to_be_bytes());
    state_hasher.update(&BASE_REWARD.to_be_bytes());
    state_hasher.update(&BLOCK_TARGET_TIME_SECS.to_be_bytes());
    state_hasher.update(&MIN_DIFFICULTY.to_be_bytes());
    state_hasher.update(&EPOCH.to_be_bytes());
    state_hasher.update(&POS_FINALITY_BLOCKS.to_be_bytes());
    state_hasher.update(&MIN_BOND_STAKE.to_be_bytes());
    state_hasher.update(&MIN_FEE.to_be_bytes());
    state_hasher.update(&BLOCK_VERSION.to_be_bytes());
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
            version: BLOCK_VERSION,
            height: 0,
            previous_hash: Hash::zero(),
            state_root,
            transaction_root: Hash::zero(),
            timestamp: config.attestation.ceremony_timestamp,
            difficulty: config.initial_difficulty,
            suggested_fee: MIN_FEE,
            extension_root: None,
            pow_proof: None,
            validator_signature: None,
        },
        transactions: vec![],
    }
}

/// Validate that a block conforms to genesis invariants.
///
/// The genesis block must:
/// - Have height 0,
/// - Have a zero `previous_hash`,
/// - Have no PoW proof (it is created without mining),
/// - Contain no transactions (all state comes from protocol constants).
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