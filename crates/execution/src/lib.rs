//! Opolys transaction execution engine.
//!
//! This crate is responsible for applying transactions to chain state. It acts
//! as the single dispatch layer between the consensus layer (which decides
//! *which* transactions to include) and the account/refiner stores (which hold
//! *what* the state looks like).
//!
//! Transaction execution reports ordinary fees separately from assay burns.
//! Mined blocks burn ordinary fees. Refiner-produced blocks pay ordinary fees
//! to the selected refiner producer. Bond/unbond assays are always burned.
//!
//! Supported transaction types:
//! - **Transfer** — move OPL between accounts
//! - **RefinerBond** — lock OPL as refiner stake (min 100 OPL)
//! - **RefinerUnbond** — release staked OPL back to the refiner's balance

pub mod dispatcher;

pub use dispatcher::*;
