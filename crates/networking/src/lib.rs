//! Opolys networking layer.
//!
//! Provides peer-to-peer communication primitives for the Opolys ($OPL) node:
//!
//! - **Gossip** — pub/sub message propagation for transactions and blocks
//! - **Sync** — block synchronization between peers
//! - **Discovery** — peer discovery and connection management

pub mod gossip;
pub mod sync;
pub mod discovery;

pub use gossip::*;
pub use sync::*;
pub use discovery::*;