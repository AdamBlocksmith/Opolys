//! Memory-fingerprinting challenge/response protocol.
//!
//! When a new full-node peer connects, we send a ChallengeRequest containing a
//! random (height, nonce) pair. Both sides bind the hash input to the
//! challenger and responder PeerIds before computing the EVO-OMAP hash and
//! returning it. Because EVO-OMAP requires the 256 MiB dataset to be loaded, a
//! peer that cannot answer correctly cannot be running the real mining software.
//!
//! The challenge uses the same `/opolys/challenge/1` request-response protocol as
//! the sync protocol. Both sides run the protocol — a node simultaneously challenges
//! incoming peers and responds to challenges from peers it dials.

use serde::{Deserialize, Serialize};

pub const CHALLENGE_PROTOCOL_NAME: &str = "/opolys/challenge/1";
pub const CHALLENGE_TIMEOUT_SECS: u64 = 15;

/// Sent by the challenging node to a newly connected peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChallengeRequest {
    /// Block height to use for epoch seed derivation.
    pub height: u64,
    /// Nonce to hash — chosen randomly by the challenger.
    pub nonce: u64,
}

/// The peer's response: just the hash value for the given (height, nonce).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChallengeResponse {
    pub hash_val: u64,
}

pub fn challenge_protocol() -> libp2p::StreamProtocol {
    libp2p::StreamProtocol::new(CHALLENGE_PROTOCOL_NAME)
}
