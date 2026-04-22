use opolys_core::KAD_BUCKET_SIZE;

pub struct DiscoveryConfig {
    pub bucket_size: usize,
    pub query_timeout_secs: u64,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        DiscoveryConfig {
            bucket_size: KAD_BUCKET_SIZE,
            query_timeout_secs: 60,
        }
    }
}