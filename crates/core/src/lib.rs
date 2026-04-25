//! Core types, constants, and error definitions for the Opolys ($OPL) blockchain.
//!
//! Opolys is a blockchain built as fully decentralized digital gold — a pure coin with
//! no tokens, assets, governance, or hardcoded fees. The block reward (BASE_REWARD = 312 OPL)
//! is derived from real-world gold production data: annual ~3,630 tonnes / ~212,000,000 troy oz,
//! divided by total above-ground gold (~219,891 tonnes). All hashing uses Blake3-256 (32 bytes).
//!
//! # Currency Units (6 decimal places)
//! - **OPL** — whole coin
//! - **Flake** — 0.000001 OPL (1,000,000 per OPL) — the only sub-unit
//!
//! Fees are market-driven and burned. The monetary model follows a natural equilibrium
//! with no hard cap — supply grows at the rate of real-world gold production.

pub mod constants;
pub mod errors;
pub mod types;

pub use constants::*;
pub use errors::*;
pub use types::*;