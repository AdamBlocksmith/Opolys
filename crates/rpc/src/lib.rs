//! Opolys JSON-RPC server.
//!
//! Exposes an HTTP-based JSON-RPC 2.0 interface for querying chain state
//! and submitting transactions and externally-mined blocks.
//!
//! Endpoints:
//! - `POST /rpc` — JSON-RPC 2.0 methods (chain queries, tx submission, mining)
//! - `GET /health` — simple health check
//!
//! The `BlockSubmission` channel allows external miners to submit blocks
//! through `opl_submitSolution` for validation and application to the chain.

pub mod server;
pub mod jsonrpc;

pub use server::*;
pub use jsonrpc::*;