//! # Opolys Consensus Engine
//!
//! This crate implements the consensus logic for the Opolys ($OPL) blockchain —
//! a fully decentralized digital gold with no hard cap. Difficulty and rewards
//! emerge organically from chain state. Fees are market-driven and burned.
//! Validators earn from block rewards only. Only double-signing gets slashed.
//! No governance, no schedules, no fixed percentages.
//!
//! ## Modules
//!
//! - **account**: Ledger accounts with balance, nonce, and transfer semantics.
//! - **block**: Block hashing, transaction roots, and formatting.
//! - **difficulty**: Adaptive difficulty retargeting, consensus floor, and PoW checks.
//! - **emission**: Block reward computation — inversely proportional to difficulty, scaled by discovery bonus.
//! - **genesis**: Genesis block construction and validation from ceremony attestation data.
//! - **mempool**: Fee-prioritized transaction pool with eviction and per-account limits.
//! - **pow**: Autolykos-inspired proof-of-work mining and verification.
//! - **pos**: Proof-of-stake validator set management — bonding, slashing, and block producer selection.

pub mod account;
pub mod block;
pub mod difficulty;
pub mod emission;
pub mod genesis;
pub mod mempool;
pub mod pow;
pub mod pos;

pub use account::*;
pub use block::*;
pub use difficulty::*;
pub use emission::*;
pub use genesis::*;
pub use mempool::*;
pub use pow::*;
pub use pos::*;