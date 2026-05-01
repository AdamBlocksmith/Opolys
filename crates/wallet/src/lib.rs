//! Opolys wallet and key management.
//!
//! This crate provides the full key derivation and transaction signing stack
//! for the Opolys ($OPL) blockchain:
//!
//! - **BIP-39** — 24-word mnemonic generation and validation (256 bits of entropy)
//! - **SLIP-0010** — Hierarchical deterministic key derivation for ed25519
//!   at path `m/44'/999'/0'/0'` (hardened-only, as required by ed25519)
//! - **Transaction building** — constructing and signing Transfer, Bond,
//!   and Unbond transactions
//! - **Account management** — named account lookup and formatting helpers
//!
//! Opolys uses Blake3-256 for hashing. Addresses (ObjectId) are derived from
//! the ed25519 public key via Blake3. The same ed25519 key is used for both
//! transaction signing and refiner block signing. Full wallet recovery from
//! mnemonic alone is supported — no separate backup file needed.

pub mod key;
pub mod bip39;
pub mod signing;
pub mod account;

pub use key::*;
pub use bip39::*;
pub use signing::*;
pub use account::*;