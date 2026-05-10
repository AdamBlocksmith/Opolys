//! Core types, constants, and error definitions for the Opolys ($OPL) blockchain.
//!
//! Opolys is a blockchain built as fully decentralized digital gold — a pure coin with
//! no tokens, assets, governance, or hardcoded fees. The fallback base reward
//! (`BASE_REWARD = 332 OPL`) is derived from real-world gold production data:
//! annual ~3,630 tonnes × 32,150.7 troy oz/tonne ≈ 116,707,041 troy oz,
//! divided across 350,640 yearly blocks. Mainnet nodes use the signed genesis
//! ceremony value stored in chain state. All hashing uses Blake3-256 (32 bytes).
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
