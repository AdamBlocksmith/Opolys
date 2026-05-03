//! Peer discovery configuration using Kademlia DHT.
//!
//! Opolys uses Kademlia for distributed peer discovery. New nodes bootstrap
//! from known peers and then use the DHT to discover additional peers
//! on the network.

use opolys_core::{KAD_BUCKET_SIZE, KAD_QUERY_TIMEOUT_SECS};

/// Configuration for Kademlia DHT peer discovery.
pub struct DiscoveryConfig {
    /// Kademlia bucket size (k-parameter).
    pub bucket_size: usize,
    /// Timeout for DHT queries in seconds.
    pub query_timeout_secs: u64,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        DiscoveryConfig {
            bucket_size: KAD_BUCKET_SIZE,
            query_timeout_secs: KAD_QUERY_TIMEOUT_SECS,
        }
    }
}

/// DNS seeds for mainnet — resolved at startup, not hardcoded IPs.
/// Update these DNS records to rotate bootstrap infrastructure without releasing a new binary.
pub const MAINNET_DNS_SEEDS: &[&str] = &["seed.opolys.io", "seed2.opolys.io", "seed3.opolys.io"];

/// Resolve DNS seed hostnames to QUIC Multiaddr strings.
///
/// Each hostname is queried on port 4170 (the default Opolys P2P port).
/// A/AAAA records are returned as `/ip4/<IP>/udp/4170/quic-v1` addresses.
/// Failures are silently skipped — DNS seeds are best-effort.
pub async fn resolve_dns_seeds(seeds: &[&str]) -> Vec<String> {
    let mut addrs = Vec::new();
    for seed in seeds {
        match tokio::net::lookup_host(format!("{}:4170", seed)).await {
            Ok(resolved) => {
                for addr in resolved {
                    if addr.ip().is_ipv4() {
                        addrs.push(format!("/ip4/{}/udp/4170/quic-v1", addr.ip()));
                    }
                }
            }
            Err(e) => {
                tracing::debug!(seed, error = %e, "DNS seed resolution failed (skipping)");
            }
        }
    }
    addrs
}
