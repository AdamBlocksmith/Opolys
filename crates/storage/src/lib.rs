//! Opolys persistent storage layer.
//!
//! Provides RocksDB-backed storage for blockchain state that must survive
//! node restarts. The database uses column families to organize data:
//!
//! - **blocks** — block data indexed by height
//! - **accounts** — all account state (balances, nonces)
//! - **refiners** — refiner set (stake, status)
//! - **chain_state** — chain metadata (height, difficulty, issued/burned totals)
//!
//! After each block is applied, all state is written to disk atomically.
//! On startup the node loads from RocksDB; if no state exists it initializes
//! from genesis.

pub mod store;

pub use store::*;