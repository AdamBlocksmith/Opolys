//! Opolys transaction execution engine.
//!
//! This crate is responsible for applying transactions to chain state. It acts
//! as the single dispatch layer between the consensus layer (which decides
//! *which* transactions to include) and the account/refiner stores (which hold
//! *what* the state looks like).
//!
//! Every transaction fee is **burned** (permanently removed from supply) —
//! refiners earn from block rewards alone, not from fees. This is a core
//! design choice of the Opolys ($OPL) blockchain: digital gold with no hard cap,
//! where difficulty and rewards emerge from chain state.
//!
//! Supported transaction types:
//! - **Transfer** — move OPL between accounts
//! - **RefinerBond** — lock OPL as refiner stake (min 100 OPL)
//! - **RefinerUnbond** — release staked OPL back to the refiner's balance

pub mod dispatcher;

pub use dispatcher::*;
