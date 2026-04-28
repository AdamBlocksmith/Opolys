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

use opolys_core::{Block, BlockHeader, Hash, ObjectId, FlakeAmount, MIN_DIFFICULTY, GENESIS_DIFFICULTY, BASE_REWARD, NETWORK_PROTOCOL_VERSION, BLOCK_TARGET_TIME_MS, MIN_BOND_STAKE, FLAKES_PER_OPL, EPOCH, POS_FINALITY_BLOCKS, BLOCK_VERSION, MIN_FEE, CURRENCY_NAME, CURRENCY_TICKER, CURRENCY_SMALLEST_UNIT};
use borsh::{BorshSerialize, BorshDeserialize};
use opolys_crypto::Blake3Hasher;
use crate::account::AccountStore;

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
///
/// Genesis accounts can be specified to pre-fund wallets for testnet.
/// Each genesis account is credited with the specified amount of Flakes
/// in the genesis state without requiring a transaction.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct GenesisConfig {
    /// The starting difficulty at height 0. Typically `MIN_DIFFICULTY`.
    pub initial_difficulty: u64,
    /// Protocol version string for the genesis block.
    pub protocol_version: String,
    /// The ceremony attestation anchoring genesis to gold data.
    pub attestation: GenesisAttestation,
    /// Genesis accounts that receive initial OPL balances and their ed25519 public keys.
    /// Each entry is (ObjectId, FlakeAmount, pubkey_bytes) — public key is 32 bytes
    /// (ed25519 verifying key). Required so accounts can sign transactions immediately
    /// without an empty-key bypass.
    pub genesis_accounts: Vec<(ObjectId, FlakeAmount, Vec<u8>)>,
}

impl Default for GenesisConfig {
    fn default() -> Self {
        GenesisConfig {
            initial_difficulty: GENESIS_DIFFICULTY,
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
                // Derivation: floor(annual_production * troy_oz_per_tonne / blocks_per_year)
                // blocks_per_year = 365.25 * 1024 = 374,256
                derivation_formula: "floor(annual_production_tonnes * 32150.7 / 374256)".to_string(),
            },
            genesis_accounts: vec![],
        }
    }
}

/// Build the Opolys genesis block from the given configuration.
///
/// The genesis block has:
/// - Height 0 and `previous_hash` set to all zeros.
/// - No PoW proof, no validator signature.
/// - A deterministic `state_root` computed by hashing all protocol constants,
///   attestation fields, and genesis account balances, ensuring every node
///   arrives at the same state.
/// - No transactions — genesis accounts are credited off-chain, not via tx.
pub fn build_genesis_block(config: &GenesisConfig) -> Block {
    // Compute the state root by hashing every protocol constant, attestation
    // field, and genesis account in a fixed order. This makes the genesis
    // state fully deterministic — any node with the same config produces
    // the exact same hash.
    let mut state_hasher = Blake3Hasher::new();
    state_hasher.update(config.protocol_version.as_bytes());
    state_hasher.update(CURRENCY_NAME.as_bytes());
    state_hasher.update(CURRENCY_TICKER.as_bytes());
    state_hasher.update(CURRENCY_SMALLEST_UNIT.as_bytes());
    state_hasher.update(&FLAKES_PER_OPL.to_be_bytes());
    state_hasher.update(&BASE_REWARD.to_be_bytes());
    state_hasher.update(&BLOCK_TARGET_TIME_MS.to_be_bytes());
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
    // Include genesis accounts in the state root for deterministic genesis
    for (account_id, amount, pk) in &config.genesis_accounts {
        state_hasher.update(account_id.as_bytes());
        state_hasher.update(&amount.to_be_bytes());
        state_hasher.update(pk.as_slice());
    }
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
            producer: ObjectId(Hash::zero()),
            pow_proof: None,
            validator_signature: None,
        },
        transactions: vec![],
    }
}

/// Apply genesis account balances to the AccountStore.
///
/// Each genesis account is credited with their initial OPL balance.
/// No transactions are created — the balances are written directly
/// into the chain state. This ensures genesis accounts exist from
/// block 0 without requiring a coinbase transaction.
pub fn apply_genesis_accounts(
    config: &GenesisConfig,
    accounts: &mut AccountStore,
) -> FlakeAmount {
    let mut total_issued: FlakeAmount = 0;
    for (account_id, amount, pk) in &config.genesis_accounts {
        if accounts.get_account(account_id).is_none() {
            accounts.create_account(account_id.clone()).ok();
        }
        if let Some(account) = accounts.get_account_mut(account_id) {
            if !pk.is_empty() {
                account.public_key = Some(pk.clone());
            }
        }
        accounts.credit(account_id, *amount).ok();
        total_issued = total_issued.saturating_add(*amount);
    }
    total_issued
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