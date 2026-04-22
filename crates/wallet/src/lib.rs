//! Opolys wallet and key management.
//!
//! This crate provides the full key derivation and transaction signing stack
//! for the Opolys ($OPL) blockchain:
//!
//! - **BIP-39** — 24-word mnemonic generation and validation (256 bits of entropy)
//! - **SLIP-0010** — Hierarchical deterministic key derivation for ed25519
//!   at path `m/44'/999'/0'/0'` (hardened-only, as required by ed25519)
//! - **Hybrid signing** — dual classical (ed25519) + quantum-resistant
//!   (Dilithium) signatures for future-proof transaction authentication
//! - **Transaction building** — constructing and signing Transfer, Bond,
//!   and Unbond transactions
//! - **Account management** — named account lookup and formatting helpers
//!
//! Opolys uses Blake3-256 for hashing. Addresses (ObjectId) are derived from
//! the ed25519 public key via Blake3. Transaction fees are market-driven and
//! burned; validators earn from block rewards only.

pub mod key;
pub mod bip39;
pub mod hybrid_keypair;
pub mod signing;
pub mod account;

pub use key::*;
pub use bip39::*;
pub use hybrid_keypair::*;
pub use signing::*;
pub use account::*;