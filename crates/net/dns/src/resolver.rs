//! Perform DNS lookups

use async_trait::async_trait;
use parking_lot::RwLock;
use std::collections::HashMap;
use tracing::trace;
pub use trust_dns_resolver::{error::ResolveError, TokioAsyncResolver};
use trust_dns_resolver::{name_server::ConnectionProvider, AsyncResolver};

/// A type that can lookup DNS entries
#[async_trait]
pub trait Resolver: Send + Sync + Unpin + 'static {
    /// Performs a textual lookup and returns the first text
    async fn lookup_txt(&self, query: &str) -> Option<String>;
}

#[async_trait]
impl<P: ConnectionProvider> Resolver for AsyncResolver<P> {
    async fn lookup_txt(&self, query: &str) -> Option<String> {
        // See: [AsyncResolver::txt_lookup]
        // > *hint* queries that end with a '.' are fully qualified names and are cheaper lookups
        let fqn = if query.ends_with('.') { query.to_string() } else { format!("{query}.") };
        match self.txt_lookup(fqn).await {
            Err(err) => {
                trace!(target: "disc::dns", ?err, ?query, "dns lookup failed");
                None
            }
            Ok(lookup) => {
                let txt = lookup.into_iter().next()?;
                let entry = txt.iter().next()?;
                String::from_utf8(entry.to_vec()).ok()
            }
        }
    }
}

/// An asynchronous DNS resolver
///
/// See also [TokioAsyncResolver]
///
/// ```
/// # fn t() {
///  use reth_dns_discovery::resolver::DnsResolver;
///  let resolver = DnsResolver::from_system_conf().unwrap();
/// # }
/// ```
///
/// Note: This [Resolver] can send multiple lookup attempts, See also
/// [ResolverOpts](trust_dns_resolver::config::ResolverOpts) which configures 2 attempts (1 retry)
/// by default.
#[derive(Clone, Debug)]
pub struct DnsResolver(TokioAsyncResolver);

// === impl DnsResolver ===

impl DnsResolver {
    /// Create a new resolver by wrapping the given [AsyncResolver]
    pub fn new(resolver: TokioAsyncResolver) -> Self {
        Self(resolver)
    }

    /// Constructs a new Tokio based Resolver with the system configuration.
    ///
    /// This will use `/etc/resolv.conf` on Unix OSes and the registry on Windows.
    pub fn from_system_conf() -> Result<Self, ResolveError> {
        TokioAsyncResolver::tokio_from_system_conf().map(Self::new)
    }
}

#[async_trait]
impl Resolver for DnsResolver {
    async fn lookup_txt(&self, query: &str) -> Option<String> {
        Resolver::lookup_txt(&self.0, query).await
    }
}

/// A [Resolver] that uses an in memory map to lookup entries
#[derive(Debug, Default)]
pub struct MapResolver(RwLock<HashMap<String, String>>);

// === impl MapResolver ===

impl MapResolver {
    /// Inserts a key-value pair into the map.
    pub fn insert(&self, k: String, v: String) -> Option<String> {
        self.0.write().insert(k, v)
    }

    /// Returns the value corresponding to the key
    pub fn get(&self, k: &str) -> Option<String> {
        self.0.read().get(k).cloned()
    }

    /// Removes a key from the map, returning the value at the key if the key was previously in the
    /// map.
    pub fn remove(&self, k: &str) -> Option<String> {
        self.0.write().remove(k)
    }
}

#[async_trait]
impl Resolver for MapResolver {
    async fn lookup_txt(&self, query: &str) -> Option<String> {
        self.get(query)
    }
}

/// A Resolver that always times out.
#[cfg(test)]
pub(crate) struct TimeoutResolver(pub(crate) std::time::Duration);

#[cfg(test)]
#[async_trait]
impl Resolver for TimeoutResolver {
    async fn lookup_txt(&self, _query: &str) -> Option<String> {
        tokio::time::sleep(self.0).await;
        None
    }
}
