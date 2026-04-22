//! Opolys transaction execution engine.
//!
//! This crate is responsible for applying transactions to chain state. It acts
//! as the single dispatch layer between the consensus layer (which decides
//! *which* transactions to include) and the account/validator stores (which hold
//! *what* the state looks like).
//!
//! Every transaction fee is **burned** (permanently removed from supply) —
//! validators earn from block rewards alone, not from fees. This is a core
//! design choice of the Opolys ($OPL) blockchain: digital gold with no hard cap,
//! where difficulty and rewards emerge from chain state.
//!
//! Supported transaction types:
//! - **Transfer** — move OPL between accounts
//! - **ValidatorBond** — lock OPL as validator stake (min 100 OPL)
//! - **ValidatorUnbond** — release staked OPL back to the validator's balance

pub mod dispatcher;

pub use dispatcher::*;