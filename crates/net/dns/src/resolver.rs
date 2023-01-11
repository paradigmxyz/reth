//! Perform DNS lookups

use async_trait::async_trait;
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
};
use tracing::trace;
pub use trust_dns_resolver::TokioAsyncResolver;
use trust_dns_resolver::{
    error::ResolveError, proto::DnsHandle, AsyncResolver, ConnectionProvider,
};

/// A type that can lookup DNS entries
#[async_trait]
pub trait Resolver: Send + Sync + Unpin + 'static {
    /// Performs a textual lookup and returns the first text
    async fn lookup_txt(&self, query: &str) -> Option<String>;
}

#[async_trait]
impl<C, P> Resolver for AsyncResolver<C, P>
where
    C: DnsHandle<Error = ResolveError>,
    P: ConnectionProvider<Conn = C>,
{
    async fn lookup_txt(&self, query: &str) -> Option<String> {
        // See: [AsyncResolver::txt_lookup]
        // > *hint* queries that end with a '.' are fully qualified names and are cheaper lookups
        let fqn = if query.ends_with('.') { query.to_string() } else { format!("{query}.") };
        match self.txt_lookup(fqn).await {
            Err(err) => {
                trace!(?err, ?query, "dns lookup failed");
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
/// See also [TokioAsyncResolver](trust_dns_resolver::TokioAsyncResolver)
///
/// ```
/// # fn t() {
///  use reth_dns_discovery::resolver::DnsResolver;
///  let resolver = DnsResolver::from_system_conf().unwrap();
/// # }
/// ```
#[derive(Clone)]
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
#[derive(Debug, Clone, Default)]
pub struct MapResolver(HashMap<String, String>);

impl Deref for MapResolver {
    type Target = HashMap<String, String>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for MapResolver {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[async_trait]
impl Resolver for MapResolver {
    async fn lookup_txt(&self, query: &str) -> Option<String> {
        self.get(query).cloned()
    }
}
