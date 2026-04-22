//! Opolys full-node implementation.
//!
//! This crate ties together consensus, execution, storage, and RPC into a
//! single running node. It manages:
//!
//! - **Chain state** — height, difficulty, issuance/burn tracking, block linkage
//! - **Mining** — Autolykos PoW mining loop for block production
//! - **Block application** — state transitions including transaction execution,
//!   fee burning, reward emission, and difficulty adjustment
//! - **Persistence** — saving and loading state via RocksDB
//! - **RPC** — serving chain queries via JSON-RPC
//!
//! The node transitions from pure PoW to blended PoW/PoS as validator stake
//! coverage grows (no governance or schedules — difficulty and rewards emerge
//! from chain state).

pub mod node;

pub use node::*;