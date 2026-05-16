//! Opolys transaction execution engine.
//!
//! This crate is responsible for applying transactions to chain state. It acts
//! as the single dispatch layer between the consensus layer (which decides
//! *which* transactions to include) and the account/refiner stores (which hold
//! *what* the state looks like).
//!
//! Base transaction fees are **burned** (permanently removed from supply);
//! explicit refiner service fees are separated from the burn path and can be
//! paid only for delivered attestation/finality service. This keeps Opolys
//! ($OPL) as digital gold with no hard cap while avoiding passive refiner yield.
//!
//! Supported transaction types:
//! - **Transfer** — move OPL between accounts
//! - **RefinerBond** — lock OPL as refiner stake (min 100 OPL)
//! - **RefinerUnbond** — release staked OPL back to the refiner's balance

pub mod dispatcher;

pub use dispatcher::*;
