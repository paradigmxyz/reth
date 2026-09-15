//! Perform DNS lookups

use crate::tree::has_entry_prefix;
use dashmap::DashMap;
pub use hickory_resolver::{net::NetError, TokioResolver};
use hickory_resolver::{
    proto::rr::{rdata::TXT, RData, Record},
    ConnectionProvider,
};
use std::future::Future;
use tracing::trace;

/// A type that can lookup DNS entries
pub trait Resolver: Send + Sync + Unpin + 'static {
    /// Performs a textual lookup and returns the first text
    fn lookup_txt(&self, query: &str) -> impl Future<Output = Option<String>> + Send;
}

impl<P: ConnectionProvider> Resolver for hickory_resolver::Resolver<P> {
    async fn lookup_txt(&self, query: &str) -> Option<String> {
        // See: [AsyncResolver::txt_lookup]
        // > *hint* queries that end with a '.' are fully qualified names and are cheaper lookups
        let fqn = if query.ends_with('.') { query.to_string() } else { format!("{query}.") };
        match self.txt_lookup(fqn).await {
            Err(err) => {
                trace!(target: "disc::dns", %err, ?query, "dns lookup failed");
                None
            }
            Ok(lookup) => find_txt_entry(lookup.answers()),
        }
    }
}

/// Returns the first TXT record with a recognized EIP-1459 entry prefix.
fn find_txt_entry(records: &[Record]) -> Option<String> {
    records
        .iter()
        .filter_map(|record| {
            let RData::TXT(txt) = &record.data else { return None };
            txt_entry(txt)
        })
        .find(|entry| has_entry_prefix(entry))
}

/// Joins all `<character-string>`s of a TXT record into a single entry.
///
/// [RFC 1035](https://www.rfc-editor.org/rfc/rfc1035#section-3.3) limits a single
/// `<character-string>` to 255 bytes, while an
/// [EIP-1459](https://eips.ethereum.org/EIPS/eip-1459) entry is only bounded by the 512 byte DNS
/// UDP limit. Entries above 255 bytes are therefore published as several `<character-string>`s
/// which have to be rejoined without a separator to recover the entry.
fn txt_entry(txt: &TXT) -> Option<String> {
    String::from_utf8(txt.txt_data.concat()).ok()
}

/// An asynchronous DNS resolver
///
/// See also [`TokioResolver`]
///
/// ```
/// # fn t() {
/// use reth_dns_discovery::resolver::DnsResolver;
/// let resolver = DnsResolver::from_system_conf().unwrap();
/// # }
/// ```
///
/// Note: This [Resolver] can send multiple lookup attempts, See also
/// [`ResolverOpts`](hickory_resolver::config::ResolverOpts) which configures 2 attempts (1 retry)
/// by default.
#[derive(Clone, Debug)]
pub struct DnsResolver(TokioResolver);

// === impl DnsResolver ===

impl DnsResolver {
    /// Create a new resolver by wrapping the given [`TokioResolver`].
    pub const fn new(resolver: TokioResolver) -> Self {
        Self(resolver)
    }

    /// Constructs a new Tokio based Resolver with the system configuration.
    ///
    /// This will use `/etc/resolv.conf` on Unix OSes and the registry on Windows.
    pub fn from_system_conf() -> Result<Self, NetError> {
        TokioResolver::builder_tokio()?.build().map(Self::new)
    }
}

impl Resolver for DnsResolver {
    async fn lookup_txt(&self, query: &str) -> Option<String> {
        Resolver::lookup_txt(&self.0, query).await
    }
}

/// A [Resolver] that uses an in memory map to lookup entries
#[derive(Debug, Default)]
pub struct MapResolver(DashMap<String, String>);

// === impl MapResolver ===

impl MapResolver {
    /// Inserts a key-value pair into the map.
    pub fn insert(&self, k: String, v: String) -> Option<String> {
        self.0.insert(k, v)
    }

    /// Returns the value corresponding to the key
    pub fn get(&self, k: &str) -> Option<String> {
        self.0.get(k).map(|entry| entry.value().clone())
    }

    /// Removes a key from the map, returning the value at the key if the key was previously in the
    /// map.
    pub fn remove(&self, k: &str) -> Option<String> {
        self.0.remove(k).map(|(_, v)| v)
    }
}

impl Resolver for MapResolver {
    async fn lookup_txt(&self, query: &str) -> Option<String> {
        self.get(query)
    }
}

/// A Resolver that always times out.
#[cfg(test)]
pub(crate) struct TimeoutResolver(pub(crate) std::time::Duration);

#[cfg(test)]
impl Resolver for TimeoutResolver {
    async fn lookup_txt(&self, _query: &str) -> Option<String> {
        tokio::time::sleep(self.0).await;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::keccak256;
    use data_encoding::BASE32_NOPAD;
    use hickory_resolver::{
        config::{ConnectionConfig, NameServerConfig, ResolverConfig},
        net::runtime::TokioRuntimeProvider,
        proto::{
            op::{Message, OpCode},
            rr::{Name, Record},
            serialize::binary::{BinDecodable, BinEncodable},
        },
    };
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::net::UdpSocket;

    /// Maximum size of a single RFC 1035 `<character-string>`.
    const MAX_CHARACTER_STRING: usize = 255;

    /// A branch entry of the size go-ethereum's writer emits, which exceeds what a single
    /// `<character-string>` can hold.
    fn long_branch_entry() -> String {
        let children = (0u8..13)
            .map(|i| BASE32_NOPAD.encode(&keccak256([i]).as_slice()[..16]))
            .collect::<Vec<_>>()
            .join(",");
        format!("enrtree-branch:{children}")
    }

    fn character_strings(entry: &str) -> Vec<String> {
        entry
            .as_bytes()
            .chunks(MAX_CHARACTER_STRING)
            .map(|chunk| String::from_utf8(chunk.to_vec()).unwrap())
            .collect()
    }

    /// Answers every query with a single TXT record made up of `strings`.
    async fn spawn_txt_server(strings: Vec<String>) -> SocketAddr {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            while let Ok((len, from)) = socket.recv_from(&mut buf).await {
                let request = Message::from_bytes(&buf[..len]).unwrap();
                let query = request.queries.first().unwrap().clone();

                let mut response = Message::response(request.metadata.id, OpCode::Query);
                response.metadata.authoritative = true;
                response.metadata.recursion_desired = request.metadata.recursion_desired;
                response.metadata.recursion_available = true;
                response.answers.push(Record::from_rdata(
                    query.name().clone(),
                    60,
                    RData::TXT(TXT::new(strings.clone())),
                ));
                response.queries.push(query);

                socket.send_to(&response.to_bytes().unwrap(), from).await.unwrap();
            }
        });
        addr
    }

    fn resolver_for(addr: SocketAddr) -> DnsResolver {
        let mut connection = ConnectionConfig::udp();
        connection.port = addr.port();
        let config = ResolverConfig::from_parts(
            None,
            vec![],
            vec![NameServerConfig::new(addr.ip(), true, vec![connection])],
        );
        DnsResolver::new(
            TokioResolver::builder_with_config(config, TokioRuntimeProvider::default())
                .build()
                .unwrap(),
        )
    }

    #[test]
    fn txt_entry_joins_character_strings() {
        let entry = long_branch_entry();
        let strings = character_strings(&entry);
        assert!(entry.len() > MAX_CHARACTER_STRING);
        assert!(strings.len() > 1);

        assert_eq!(txt_entry(&TXT::new(strings)), Some(entry));
    }

    #[tokio::test]
    async fn lookup_txt_reads_record_split_over_character_strings() {
        let entry = long_branch_entry();
        let addr = spawn_txt_server(character_strings(&entry)).await;

        let resolved = resolver_for(addr).lookup_txt("YNEGZIWHOM7TOOSUATAPTM.example.org").await;

        assert_eq!(resolved, Some(entry));
    }

    fn txt_record(entry: &str) -> Record {
        Record::from_rdata(Name::root(), 60, RData::TXT(TXT::new(vec![entry.to_string()])))
    }

    #[test]
    fn find_txt_entry_skips_unrelated_records() {
        let entry = "enrtree-root:v1 e=enr-root l=link-root seq=1 sig=signature";
        let records = [
            txt_record("v=spf1 -all"),
            txt_record(entry),
            txt_record("enrtree-branch:YNEGZIWHOM7TOOSUATAPTM"),
        ];

        assert_eq!(find_txt_entry(&records).as_deref(), Some(entry));
    }
}
