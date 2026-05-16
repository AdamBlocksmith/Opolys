//! Opolys full-node implementation.
//!
//! This crate ties together consensus, execution, storage, and RPC into a
//! single running node. It manages:
//!
//! - **Chain state** — height, difficulty, issuance/burn tracking, block linkage
//! - **Mining** -- EVO-OMAP PoW mining loop for block production
//! - **Block application** -- transaction execution, fee routing, reward emission,
//!   and difficulty adjustment
//! - **Persistence** — saving and loading state via RocksDB
//! - **RPC** — serving chain queries via JSON-RPC
//!
//! Refiners produce blocks only when miners stall. Mined-block fees burn;
//! refiner-block fees pay the selected refiner producer.

pub mod node;

pub use node::*;
