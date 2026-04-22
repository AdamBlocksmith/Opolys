use thiserror::Error;

#[derive(Error, Debug)]
pub enum OpolysError {
    #[error("Insufficient balance: need {need}, have {have}")]
    InsufficientBalance { need: u64, have: u64 },

    #[error("Invalid nonce: expected {expected}, got {got}")]
    InvalidNonce { expected: u64, got: u64 },

    #[error("Invalid signature")]
    InvalidSignature,

    #[error("Invalid proof of work")]
    InvalidProofOfWork,

    #[error("Account not found: {0}")]
    AccountNotFound(String),

    #[error("Validator not found: {0}")]
    ValidatorNotFound(String),

    #[error("Insufficient stake: need {need}, have {have}")]
    InsufficientStake { need: u64, have: u64 },

    #[error("Validator already bonded")]
    ValidatorAlreadyBonded,

    #[error("Validator not bonded")]
    ValidatorNotBonded,

    #[error("Invalid transaction: {0}")]
    InvalidTransaction(String),

    #[error("Block validation failed: {0}")]
    BlockValidationFailed(String),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Mempool full")]
    MempoolFull,

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Genesis error: {0}")]
    GenesisError(String),

    #[error("{0}")]
    Custom(String),
}