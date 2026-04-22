//! Cryptographic primitives for the Opolys ($OPL) blockchain.
//!
//! Opolys is fully decentralized digital gold — a pure coin with no tokens,
//! assets, governance, or hardcoded fees. This crate provides the core crypto
//! building blocks that underpin the network:
//!
//! - **Blake3-256 hashing** (32 bytes) — used everywhere for `Hash` and `ObjectId`
//! - **ed25519 signing** — wallet transaction signing via BIP39 24-word mnemonics
//!   with SLIP-0010 ed25519 derivation
//!
//! All cryptographic operations are deterministic, side-effect-free, and
//! validation-focused (verify, don't trust).

pub mod hash;
pub mod signing;

pub use hash::*;
pub use signing::*;