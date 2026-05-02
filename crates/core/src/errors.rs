//! Error types for the Opolys ($OPL) blockchain.
//!
//! This module defines a single exhaustive error enum ([`OpolysError`]) that
//! covers all failure modes across consensus validation, account operations,
//! networking, storage, and mempool logic. Using a unified enum rather than
//! per-module error types simplifies error propagation across crate boundaries
//! while still providing structured, typed context via variants.

use thiserror::Error;

/// The canonical error type for all Opolys operations.
///
/// Every subsystem (validation, storage, networking, mempool) returns this enum
/// so that callers can pattern-match on specific failure modes or bubble errors
/// up with `?`.
#[derive(Error, Debug)]
pub enum OpolysError {
    /// The sender's account does not have enough Flakes to cover the
    /// transfer amount plus the fee.
    ///
    /// `need` is the total required (amount + fee), `have` is the current balance.
    #[error("Insufficient balance: need {need}, have {have}")]
    InsufficientBalance { need: u64, have: u64 },

    /// The transaction nonce does not match the expected value for this account.
    ///
    /// Expected nonce = account's current nonce + 1.
    #[error("Invalid nonce: expected {expected}, got {got}")]
    InvalidNonce { expected: u64, got: u64 },

    /// The transaction signature could not be verified against the sender's public key.
    #[error("Invalid signature")]
    InvalidSignature,

    /// The proof-of-work solution does not meet the current difficulty target.
    #[error("Invalid proof of work")]
    InvalidProofOfWork,

    /// No account exists in the state for the given identifier.
    #[error("Account not found: {0}")]
    AccountNotFound(String),

    /// No refiner entry exists in the state for the given identifier.
    #[error("Refiner not found: {0}")]
    RefinerNotFound(String),

    /// The refiner's stake is below the minimum required for the requested operation.
    ///
    /// `need` is the minimum stake (e.g., `MIN_BOND_STAKE`), `have` is the current stake.
    #[error("Insufficient stake: need {need}, have {have}")]
    InsufficientStake { need: u64, have: u64 },

    /// The account is already bonded as a refiner. Must unbond first.
    #[error("Refiner already bonded")]
    RefinerAlreadyBonded,

    /// Attempted an unbond or slash operation on an account that is not a refiner.
    #[error("Refiner not bonded")]
    RefinerNotBonded,

    /// The transaction fee is below the minimum required.
    #[error("Invalid params: {0}")]
    InvalidParams(String),

    /// Generic transaction validation failure (e.g., malformed data, wrong action).
    #[error("Invalid transaction: {0}")]
    InvalidTransaction(String),

    /// Block-level validation failure (e.g., bad state root, mismatched difficulty).
    #[error("Block validation failed: {0}")]
    BlockValidationFailed(String),

    /// An underlying storage layer error (e.g., database I/O or corruption).
    #[error("Storage error: {0}")]
    StorageError(String),

    /// A peer-to-peer networking error (e.g., connection dropped, timeout).
    #[error("Network error: {0}")]
    NetworkError(String),

    /// The mempool is at capacity and cannot accept more transactions.
    #[error("Mempool full")]
    MempoolFull,

    /// Failure to serialize or deserialize a structure (e.g., Borsh or JSON error).
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Failure during genesis block initialization (e.g., invalid config, bad allocation).
    #[error("Genesis error: {0}")]
    GenesisError(String),

    /// Catch-all variant for errors that don't fit a specific category.
    #[error("{0}")]
    Custom(String),
}
