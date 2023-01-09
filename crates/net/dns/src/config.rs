use std::time::Duration;

/// Settings for the [DnsDiscoveryClient](crate::DnsDiscoveryClient).
#[derive(Debug, Clone)]
pub struct DnsDiscoveryConfig {
    /// Timeout for DNS lookups.
    ///
    /// Default: 5s
    pub lookup_timeout: Duration,
    /// The rate at which lookups should be re-triggered.
    ///
    /// Default: 30min
    pub lookup_interval: Duration,
}

impl Default for DnsDiscoveryConfig {
    fn default() -> Self {
        Self {
            lookup_timeout: Duration::from_secs(5),
            lookup_interval: Duration::from_secs(60 * 30),
        }
    }
}
