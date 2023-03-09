use crate::tree::LinkEntry;
use std::{
    collections::HashSet,
    num::{NonZeroU32, NonZeroUsize},
    time::Duration,
};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Settings for the [DnsDiscoveryService](crate::DnsDiscoveryService).
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DnsDiscoveryConfig {
    /// Timeout for DNS lookups.
    ///
    /// Default: 5s
    pub lookup_timeout: Duration,
    /// The DNS request rate limit
    ///
    /// Default: 3
    pub max_requests_per_sec: NonZeroUsize,
    /// The rate at which trees should be updated.
    ///
    /// Default: 30min
    pub recheck_interval: Duration,
    /// Maximum number of cached DNS records.
    pub dns_record_cache_limit: NonZeroU32,
    /// Links to the DNS networks to bootstrap.
    pub bootstrap_dns_networks: Option<HashSet<LinkEntry>>,
}

impl Default for DnsDiscoveryConfig {
    fn default() -> Self {
        Self {
            lookup_timeout: Duration::from_secs(5),
            max_requests_per_sec: NonZeroUsize::new(3).unwrap(),
            recheck_interval: Duration::from_secs(60 * 30),
            dns_record_cache_limit: NonZeroU32::new(1_000).unwrap(),
            bootstrap_dns_networks: Some(Default::default()),
        }
    }
}
