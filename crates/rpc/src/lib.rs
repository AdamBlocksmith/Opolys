//! Opolys JSON-RPC server.
//!
//! Exposes an HTTP-based JSON-RPC 2.0 interface for querying chain state.
//! All queries are read-only — transactions are submitted via the gossip
//! network, not through RPC.
//!
//! Endpoints:
//! - `POST /rpc` — JSON-RPC 2.0 methods (block height, chain info, balances, accounts)
//! - `GET /health` — simple health check
//!
//! Includes rate limiting to protect against abuse.

pub mod server;
pub mod jsonrpc;

pub use server::*;
pub use jsonrpc::*;