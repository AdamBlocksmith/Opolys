//! Core type definitions for the Opolys ($OPL) blockchain.
//!
//! This module defines the fundamental data types used throughout the Opolys ecosystem:
//! - **Primitives**: Amounts, block heights, public keys, and signatures
//! - **Hash**: A Blake3-256 hash wrapper with serialization support
//! - **ObjectId**: A typed identifier for accounts and entities on-chain
//! - **TransactionAction**: The payload enum describing what a transaction does
//! - **ConsensusPhase & ValidatorStatus**: Consensus and staking state machines
//! - **BlockHeader, Transaction, Block**: The wire-format ledger structures
//! - **Currency conversion helpers**: Functions to convert between OPL sub-units
//!
//! All hashes in Opolys are 32 bytes (Blake3-256). Amounts are represented as
//! `FlakeAmount` (u64) in the smallest unit (Flake), with 6 decimal places per OPL.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Smallest indivisible unit of currency (1 OPL = 1,000,000 Flakes).
///
/// All on-chain amounts are stored and computed in Flakes to avoid floating-point error.
/// 1 Flake = 0.000001 OPL.
pub type FlakeAmount = u64;

/// Block height — the 0-indexed sequence number of a block in the chain.
///
/// Genesis block has height 0. Each subsequent block increments by 1.
pub type BlockHeight = u64;

/// Public key bytes for a validator or account.
///
/// In Opolys this is a raw byte vector whose interpretation depends on the
/// cryptographic scheme in use (e.g., Ed25519).
pub type PublicKey = Vec<u8>;

/// Cryptographic signature bytes.
///
/// Produced by signing a transaction hash with the sender's private key.
pub type Signature = Vec<u8>;

/// A Blake3-256 hash (32 bytes) used throughout Opolys for block hashes,
/// transaction IDs, Merkle roots, and state roots.
///
/// Wraps a fixed-size byte array rather than a `Vec<u8>` to enforce the 32-byte
/// invariant at the type level.
#[derive(Debug, Clone, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize)]
pub struct Hash(pub [u8; 32]);

// Serde: serialize as lowercase hex string for human readability (JSON, REST, etc.)
impl Serialize for Hash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

// Serde: deserialize from hex string, rejecting anything that isn't exactly 32 bytes.
impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <String as Deserialize>::deserialize(deserializer)?;
        let bytes = hex::decode(s).map_err(serde::de::Error::custom)?;
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom("Hash must be 32 bytes"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Hash(arr))
    }
}

impl Hash {
    /// Returns the all-zero hash, used as a sentinel value (e.g., genesis previous_hash).
    pub fn zero() -> Self {
        Hash([0u8; 32])
    }

    /// Returns `true` if every byte is zero.
    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 32]
    }

    /// Returns a reference to the inner 32-byte array.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Returns the hash as a 64-character lowercase hex string.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Constructs a `Hash` from a raw 32-byte array.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Hash(bytes)
    }
}

/// A unique on-chain identifier for an account, transaction, or other entity.
///
/// Wraps a [`Hash`] to provide type safety — an `ObjectId` is conceptually distinct
/// from a raw content hash even though they share the same 32-byte representation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct ObjectId(pub Hash);

impl ObjectId {
    /// Returns the all-zero ObjectId, used as a sentinel (e.g., coinbase sender).
    pub fn zero() -> Self {
        ObjectId(Hash::zero())
    }

    /// Parse an ObjectId from a 64-character lowercase hex string.
    ///
    /// Returns an error if the string is not exactly 64 hex characters
    /// or contains invalid hex bytes.
    pub fn from_hex(hex: &str) -> Result<Self, String> {
        if hex.len() != 64 {
            return Err(format!("ObjectId hex must be 64 characters, got {}", hex.len()));
        }
        let bytes = hex::decode(hex).map_err(|e| format!("Invalid hex: {}", e))?;
        if bytes.len() != 32 {
            return Err(format!("ObjectId must be 32 bytes, got {}", bytes.len()));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(ObjectId(Hash(arr)))
    }

    /// Returns the ObjectId as a 64-character lowercase hex string.
    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }

    /// Returns a reference to the inner 32-byte array.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0 .0
    }
}

/// The action payload of a transaction.
///
/// Each variant carries all data needed to execute it:
/// - **Transfer**: move OPL from sender to recipient; the fee is burned
/// - **ValidatorBond**: lock `amount` OPL as stake (new entry or top-up)
/// - **ValidatorUnbond**: unbond `amount` OPL using FIFO order (oldest first)
///
/// Validators can hold multiple bond entries, each with its own stake, seniority
/// clock, and bond_id. Top-up bonding creates a new entry; unbonding follows FIFO
/// order — the oldest entries are consumed first. If the unbond amount exceeds
/// an oldest entry's stake, that entry is fully consumed and the remainder comes
/// from the next oldest. Split entries keep their original `bonded_at_timestamp`
/// for weight calculation. There are no pool primitives in the protocol — pools
/// are a market innovation built on top of per-entry bonds.
///
/// Invalid bond amounts (below MIN_FEE floor) result in no fee burn and no
/// nonce advance.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransactionAction {
    /// Transfer `amount` Flakes from the sender to `recipient`.
    /// The attached `fee` (set on the Transaction itself) is burned, not collected.
    Transfer { recipient: ObjectId, amount: FlakeAmount },

    /// Bond `amount` Flakes as validator stake. If the sender is already a
    /// validator, this creates a new bond entry (top-up) with its own seniority
    /// clock. If not, this creates the validator with their first bond entry.
    /// Each entry requires `>= MIN_BOND_STAKE` (1 OPL) for new entries only.
    ValidatorBond { amount: FlakeAmount },

    /// Unbond `amount` Flakes from the validator's stake using FIFO order.
    /// The oldest bond entries are consumed first. If the amount exceeds an
    /// entry's stake, that entry is fully consumed and the remainder comes from
    /// the next oldest. Split entries keep their original timestamp for weight.
    /// The fee is burned. After a UNBONDING_DELAY_BLOCKS delay, the unbonded
    /// stake is returned to the sender. Unbonding stake still earns rewards
    /// during the delay period.
    ValidatorUnbond { amount: FlakeAmount },
}

/// The current consensus phase for a given block height.
///
/// Opolys uses a hybrid PoW/PoS model:
/// - **ProofOfWork**: miners compete to find a valid nonce (difficult block heights)
/// - **ProofOfStake**: validators take turns producing blocks (easier block heights)
///
/// The phase is deterministic based on block height and difficulty.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConsensusPhase {
    /// Miners produce this block by finding a hash below the difficulty target.
    ProofOfWork,
    /// A selected validator produces this block by signing it.
    ProofOfStake,
}

/// The bonding status of a validator account.
///
/// Transitions:
/// - `None` → `Bonding` (on `ValidatorBond` tx)
/// - `Bonding` → `Active` (once the validator is included in the active set)
/// - `Active` → `Unbonding` (on `ValidatorUnbond` tx, instant removal)
/// - Any → `Slashed` (if the validator signs conflicting blocks)
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize, PartialEq, Eq)]
pub enum ValidatorStatus {
    /// The account is not a validator.
    None,
    /// The validator has bonded stake but is not yet in the active set.
    Bonding,
    /// The validator is actively producing and attesting blocks.
    Active,
    /// The validator has unbonded and stake is being returned.
    Unbonding,
    /// The validator was caught signing conflicting blocks; stake is forfeited.
    Slashed,
}

/// Header metadata for a block — everything that is hashed to produce the block hash.
///
/// The header is separated from transaction data so that validators can verify
/// proof-of-work and state roots without downloading the full block body.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BlockHeader {
    /// Protocol version number. Allows future upgrades while maintaining
    /// backward compatibility. Version 1 is the initial protocol with
    /// EVO-OMAP PoW and ed25519 signatures.
    pub version: u32,
    /// The sequential height of this block (0 for genesis).
    pub height: BlockHeight,
    /// The hash of the previous block's header. `Hash::zero()` for genesis.
    pub previous_hash: Hash,
    /// Merkle root of the post-execution application state (accounts, stakes, etc.).
    pub state_root: Hash,
    /// Merkle root of all transactions in this block's body.
    pub transaction_root: Hash,
    /// Unix timestamp (seconds since epoch) when the block was produced.
    pub timestamp: u64,
    /// The difficulty target for this block — lower values mean harder PoW.
    pub difficulty: u64,
    /// Suggested fee in Flakes for the next block. Computed as an EMA of the
    /// previous block's transaction fees, floored at MIN_FEE (1 Flake).
    /// Miners/validators should include this in the block template so wallets
    /// can estimate appropriate fees, but any fee >= MIN_FEE is valid.
    pub suggested_fee: FlakeAmount,
    /// Optional Merkle root of extension data (e.g., rollup anchors).
    /// `None` for normal blocks. Reserved for future protocol extensions.
    pub extension_root: Option<Hash>,
    /// The ObjectId of the block producer — the miner (PoW) or validator (PoS)
    /// who earns the block reward. Set to `ObjectId::zero()` for the genesis block.
    /// In PoW mode, this is the miner's address. In PoS mode, this is the
    /// validator's on-chain identity.
    pub producer: ObjectId,
    /// The nonce/solution that satisfies the difficulty target (present in PoW phases).
    /// `None` for PoS blocks and the genesis block.
    pub pow_proof: Option<Vec<u8>>,
    /// The validator's ed25519 signature over the block hash (present in PoS phases).
    /// `None` for PoW blocks and the genesis block.
    pub validator_signature: Option<Vec<u8>>,
}

/// A signed transaction that transitions the ledger state.
///
/// Every transaction carries a fee (in Flakes) that is **burned**, not collected.
/// This implements Opolys' market-driven fee model — users set whatever fee they
/// choose, and the fee permanently removes OPL from circulation.
///
/// Signature verification flow:
/// 1. Check `Blake3(public_key) == sender` (binds the key to the identity)
/// 2. Check `tx_id == compute_tx_id(sender, action, fee, nonce)` (integrity)
/// 3. Check `ed25519_verify(signed_data, signature, public_key)` (authenticity)
///
/// The `signed_data` is `borsh::to_vec((sender, action, fee, nonce))`.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Transaction {
    /// Unique identifier for this transaction (hash of its content).
    pub tx_id: ObjectId,
    /// The account sending this transaction (pays the fee, signs the tx).
    /// Must equal `Blake3(public_key)` for the signature to be valid.
    pub sender: ObjectId,
    /// The action this transaction performs (transfer, bond, or unbond).
    pub action: TransactionAction,
    /// Fee in Flakes — burned permanently, incentivizing miners/validators to include this tx.
    pub fee: FlakeAmount,
    /// Cryptographic signature over the transaction body by the sender.
    pub signature: Vec<u8>,
    /// Signature type: 0 = ed25519 (currently the only supported type).
    /// Reserved for post-quantum signatures in future protocol versions.
    pub signature_type: u8,
    /// Sender's nonce for replay protection — must equal the account's current nonce.
    pub nonce: u64,
    /// Arbitrary data attachment (e.g., memos). Not interpreted by consensus.
    pub data: Vec<u8>,
    /// The ed25519 public key of the sender (32 bytes).
    /// Required for signature verification. Must satisfy `Blake3(public_key) == sender`.
    /// This is the raw compressed point, not the ObjectId (which is the hash of it).
    pub public_key: Vec<u8>,
}

/// A complete block: header + ordered list of transactions.
///
/// Blocks are the atomic unit of the chain. The first transaction in a block
/// is always a coinbase (reward) transaction crediting the block producer
/// with `BASE_REWARD` Flakes plus any burned fees.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Block {
    /// The header containing consensus-critical metadata.
    pub header: BlockHeader,
    /// The ordered list of transactions in this block.
    pub transactions: Vec<Transaction>,
}

/// Converts whole OPL to Flakes (the smallest on-chain unit).
///
/// Uses saturating multiplication — if the result overflows u64, it returns `u64::MAX`
/// rather than panicking.
///
/// # Examples
///
/// ```
/// use opolys_core::opl_to_flake;
/// assert_eq!(opl_to_flake(1), 1_000_000);
/// assert_eq!(opl_to_flake(440), 440_000_000);
/// ```
pub fn opl_to_flake(opl: u64) -> FlakeAmount {
    opl.saturating_mul(crate::FLAKES_PER_OPL)
}

/// Converts Flakes to whole OPL, truncating the fractional part.
///
/// This is a lossy conversion — remainder Flakes are discarded.
///
/// # Examples
///
/// ```
/// use opolys_core::flake_to_opl;
/// assert_eq!(flake_to_opl(1_000_000), 1);
/// assert_eq!(flake_to_opl(1_499_999), 1);
/// ```
pub fn flake_to_opl(flakes: FlakeAmount) -> u64 {
    flakes / crate::FLAKES_PER_OPL
}

/// Formats a Flake amount as a human-readable string with 6 decimal places.
///
/// # Examples
///
/// ```
/// use opolys_core::format_flake_as_opl;
/// assert_eq!(format_flake_as_opl(1_000_000), "1.000000 OPL");
/// assert_eq!(format_flake_as_opl(1), "0.000001 OPL");
/// assert_eq!(format_flake_as_opl(440 * 1_000_000), "440.000000 OPL");
/// ```
pub fn format_flake_as_opl(flakes: u64) -> String {
    let opl = flakes / crate::FLAKES_PER_OPL;
    let frac = flakes % crate::FLAKES_PER_OPL;
    format!("{}.{:06} OPL", opl, frac)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FLAKES_PER_OPL;

    #[test]
    fn hash_zero_is_deterministic() {
        let h1 = Hash::zero();
        let h2 = Hash::zero();
        assert_eq!(h1, h2);
        assert_eq!(h1.0.len(), 32);
    }

    #[test]
    fn opl_flake_roundtrip() {
        assert_eq!(opl_to_flake(1), 1_000_000);
        assert_eq!(flake_to_opl(opl_to_flake(1)), 1);
        assert_eq!(opl_to_flake(0), 0);
    }

    #[test]
    fn transaction_action_variants() {
        let _transfer = TransactionAction::Transfer {
            recipient: ObjectId::zero(),
            amount: 100,
        };
        let bond = TransactionAction::ValidatorBond { amount: 10_000_000 };
        let unbond = TransactionAction::ValidatorUnbond { amount: 5_000_000 };
        assert_ne!(bond, unbond);
    }

    #[test]
    fn hash_hex_roundtrip() {
        let h = Hash::from_bytes([42u8; 32]);
        let json = serde_json::to_string(&h).unwrap();
        let h2: Hash = serde_json::from_str(&json).unwrap();
        assert_eq!(h, h2);
        assert_eq!(h.to_hex().len(), 64);
    }

    #[test]
    fn format_flake_amounts() {
        assert_eq!(format_flake_as_opl(1_000_000), "1.000000 OPL");
        assert_eq!(format_flake_as_opl(0), "0.000000 OPL");
        assert_eq!(format_flake_as_opl(1), "0.000001 OPL");
        assert_eq!(format_flake_as_opl(440 * 1_000_000), "440.000000 OPL");
    }
}