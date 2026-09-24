//! Helpers for resolving the external IP.
//!
//! ## Feature Flags
//!
//! - `serde` (default): Enable serde support

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod net_if;

pub use net_if::{NetInterfaceError, DEFAULT_NET_IF_NAME};

use std::{
    fmt,
    future::{poll_fn, Future},
    net::{AddrParseError, IpAddr, ToSocketAddrs},
    pin::Pin,
    str::FromStr,
    task::{Context, Poll},
    time::Duration,
};
use tracing::debug;

use crate::net_if::resolve_net_if_ip;
#[cfg(feature = "serde")]
use serde_with::{DeserializeFromStr, SerializeDisplay};

/// URLs to `GET` the external IP address.
///
/// Taken from: <https://stackoverflow.com/questions/3253701/get-public-external-ip-address>
const EXTERNAL_IP_APIS: &[&str] =
    &["https://ipinfo.io/ip", "https://icanhazip.com", "https://ifconfig.me"];

/// All builtin resolvers.
#[derive(Debug, Clone, Eq, PartialEq, Default, Hash)]
#[cfg_attr(feature = "serde", derive(SerializeDisplay, DeserializeFromStr))]
pub enum NatResolver {
    /// Resolve with any available resolver.
    #[default]
    Any,
    /// Resolve external IP via `UPnP`.
    Upnp,
    /// Resolve external IP via a network request.
    PublicIp,
    /// Use the given [`IpAddr`]
    ExternalIp(IpAddr),
    /// Use the given domain name as the external address to expose to peers.
    /// This is behaving essentially the same as [`NatResolver::ExternalIp`], but supports domain
    /// names. Domain names are resolved to IP addresses using the OS's resolver. The first IP
    /// address found is used.
    /// This may be useful in docker bridge networks where containers are usually queried by DNS
    /// instead of direct IP addresses.
    /// Note: the domain shouldn't include a port number. Only the IP address is resolved.
    ExternalAddr(String),
    /// Resolve external IP via the network interface.
    NetIf,
    /// Resolve nothing
    None,
}

impl NatResolver {
    /// Attempts to produce an IP address (best effort).
    ///
    /// # Panics
    ///
    /// Network and interface resolution require an active Tokio runtime. Only
    /// [`Self::ExternalIp`] and [`Self::None`] are guaranteed to work without one.
    pub async fn external_addr(self) -> Option<IpAddr> {
        external_addr_with(self).await
    }

    /// Returns the fixed ip, if it is [`NatResolver::ExternalIp`] or [`NatResolver::ExternalAddr`].
    ///
    /// In the case of [`NatResolver::ExternalAddr`], it will return the first IP address found for
    /// the domain. This performs blocking DNS resolution; async callers should use
    /// [`Self::external_addr`] instead.
    pub fn as_external_ip(self, port: u16) -> Option<IpAddr> {
        match self {
            Self::ExternalIp(ip) => Some(ip),
            Self::ExternalAddr(domain) => format!("{domain}:{port}")
                .to_socket_addrs()
                .ok()
                .and_then(|mut addrs| addrs.next().map(|addr| addr.ip())),
            _ => None,
        }
    }
}

impl fmt::Display for NatResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Any => f.write_str("any"),
            Self::Upnp => f.write_str("upnp"),
            Self::PublicIp => f.write_str("publicip"),
            Self::ExternalIp(ip) => write!(f, "extip:{ip}"),
            Self::ExternalAddr(domain) => write!(f, "extaddr:{domain}"),
            Self::NetIf => f.write_str("netif"),
            Self::None => f.write_str("none"),
        }
    }
}

/// Error when parsing a [`NatResolver`]
#[derive(Debug, thiserror::Error)]
pub enum ParseNatResolverError {
    /// Failed to parse provided IP
    #[error(transparent)]
    AddrParseError(#[from] AddrParseError),
    /// Failed to parse due to unknown variant
    #[error("Unknown Nat Resolver variant: {0}")]
    UnknownVariant(String),
}

impl FromStr for NatResolver {
    type Err = ParseNatResolverError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let r = match s {
            "any" => Self::Any,
            "upnp" => Self::Upnp,
            "none" => Self::None,
            "publicip" | "public-ip" => Self::PublicIp,
            "netif" => Self::NetIf,
            s => {
                if let Some(ip) = s.strip_prefix("extip:") {
                    Self::ExternalIp(ip.parse()?)
                } else if let Some(domain) = s.strip_prefix("extaddr:") {
                    Self::ExternalAddr(domain.to_string())
                } else {
                    return Err(ParseNatResolverError::UnknownVariant(format!(
                        "Unknown Nat Resolver: {s}"
                    )));
                }
            }
        };
        Ok(r)
    }
}

/// With this type you can resolve the external public IP address on an interval basis.
///
/// Keeps at most one resolution in flight and skips missed interval ticks.
#[must_use = "Does nothing unless polled"]
pub struct ResolveNatInterval {
    resolver: NatResolver,
    future: Option<Pin<Box<dyn Future<Output = Option<IpAddr>> + Send>>>,
    interval: tokio::time::Interval,
}

impl fmt::Debug for ResolveNatInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolveNatInterval")
            .field("resolver", &self.resolver)
            .field("future", &self.future.as_ref().map(drop))
            .field("interval", &self.interval)
            .finish()
    }
}

impl ResolveNatInterval {
    fn with_interval(resolver: NatResolver, mut interval: tokio::time::Interval) -> Self {
        // Resolving once is sufficient after a delay; do not replay missed attempts in a burst.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Self { resolver, future: None, interval }
    }

    /// Creates a new [`ResolveNatInterval`] that attempts to resolve the public IP with interval of
    /// period. See also [`tokio::time::interval`]
    #[track_caller]
    pub fn interval(resolver: NatResolver, period: Duration) -> Self {
        let interval = tokio::time::interval(period);
        Self::with_interval(resolver, interval)
    }

    /// Creates a new [`ResolveNatInterval`] that attempts to resolve the public IP with interval of
    /// period with the first attempt starting at `start`. See also [`tokio::time::interval_at`]
    #[track_caller]
    pub fn interval_at(
        resolver: NatResolver,
        start: tokio::time::Instant,
        period: Duration,
    ) -> Self {
        let interval = tokio::time::interval_at(start, period);
        Self::with_interval(resolver, interval)
    }

    /// Returns the resolver used by this interval
    pub const fn resolver(&self) -> &NatResolver {
        &self.resolver
    }

    /// Completes when the next [`IpAddr`] in the interval has been reached.
    pub async fn tick(&mut self) -> Option<IpAddr> {
        poll_fn(|cx| self.poll_tick(cx)).await
    }

    /// Polls for the next resolved [`IpAddr`] in the interval to be reached.
    ///
    /// This method can return the following values:
    ///
    ///  * `Poll::Pending` if the next [`IpAddr`] has not yet been resolved.
    ///  * `Poll::Ready(Option<IpAddr>)` if the next [`IpAddr`] has been resolved. This returns
    ///    `None` if the attempt was unsuccessful.
    pub fn poll_tick(&mut self, cx: &mut Context<'_>) -> Poll<Option<IpAddr>> {
        // Dropping a resolution future cannot cancel blocking work it has already started.
        if self.interval.poll_tick(cx).is_ready() && self.future.is_none() {
            self.future = Some(Box::pin(self.resolver.clone().external_addr()));
        }

        if let Some(mut fut) = self.future.take() {
            match fut.as_mut().poll(cx) {
                Poll::Ready(ip) => return Poll::Ready(ip),
                Poll::Pending => self.future = Some(fut),
            }
        }

        Poll::Pending
    }
}

/// Attempts to produce an IP address with all builtin resolvers (best effort).
///
/// # Panics
///
/// Panics if polled outside a Tokio runtime.
pub async fn external_ip() -> Option<IpAddr> {
    external_addr_with(NatResolver::Any).await
}

/// Given a [`NatResolver`] attempts to produce an IP address (best effort).
///
/// # Panics
///
/// Network and interface resolution require an active Tokio runtime. Only
/// [`NatResolver::ExternalIp`] and [`NatResolver::None`] are guaranteed to work without one.
pub async fn external_addr_with(resolver: NatResolver) -> Option<IpAddr> {
    match resolver {
        NatResolver::Any | NatResolver::Upnp | NatResolver::PublicIp => resolve_external_ip().await,
        NatResolver::ExternalIp(ip) => Some(ip),
        NatResolver::NetIf => tokio::task::spawn_blocking(|| {
            resolve_net_if_ip(DEFAULT_NET_IF_NAME)
        })
        .await
        .inspect_err(|err| {
            debug!(target: "net::nat", %err, "Failed to join network interface resolution task");
        })
        .ok()?
        .inspect_err(|err| {
            debug!(target: "net::nat",
                 %err,
                "Failed to resolve network interface IP"
            );
        })
        .ok(),
        NatResolver::ExternalAddr(domain) => tokio::net::lookup_host(format!("{domain}:0"))
            .await
            .inspect_err(|err| {
                debug!(target: "net::nat", %err, %domain, "Failed to resolve external address");
            })
            .ok()
            .and_then(|mut addrs| addrs.next().map(|addr| addr.ip())),
        NatResolver::None => None,
    }
}

async fn resolve_external_ip() -> Option<IpAddr> {
    // Client setup can read system proxy and TLS configuration. Keep it off the task polling
    // discovery; the requests themselves use async I/O and share the same client.
    let client = tokio::task::spawn_blocking(|| {
        reqwest::Client::builder().timeout(Duration::from_secs(10)).build()
    })
    .await
    .inspect_err(|err| {
        debug!(target: "net::nat", %err, "Failed to join external IP client setup task");
    })
    .ok()?
    .inspect_err(|err| {
        debug!(target: "net::nat", %err, "Failed to build external IP client");
    })
    .ok()?;
    let futures =
        EXTERNAL_IP_APIS.iter().map(|url| resolve_external_ip_url_res(&client, url)).map(Box::pin);
    futures_util::future::select_ok(futures)
        .await
        .inspect_err(|err| {
            debug!(target: "net::nat",
            ?err,
                external_ip_apis=?EXTERNAL_IP_APIS,
                "Failed to resolve external IP from any API");
        })
        .ok()
        .map(|(ip, _)| ip)
}

async fn resolve_external_ip_url_res(client: &reqwest::Client, url: &str) -> Result<IpAddr, ()> {
    resolve_external_ip_url(client, url).await.ok_or(())
}

async fn resolve_external_ip_url(client: &reqwest::Client, url: &str) -> Option<IpAddr> {
    let response = client.get(url).send().await.ok()?;
    let response = response.error_for_status().ok()?;
    let text = response.text().await.ok()?;
    let ip = text.trim().parse().ok()?;
    if !is_public_ip(ip) {
        debug!(target: "net::nat", %ip, %url, "Ignoring non-public IP from external IP API");
        return None;
    }
    Some(ip)
}

/// Filters HTTP provider results only; configured and interface addresses may be private.
///
/// Uses the IANA special-purpose registries until `IpAddr::is_global` is stable.
/// <https://www.iana.org/assignments/iana-ipv4-special-registry/>
/// <https://www.iana.org/assignments/iana-ipv6-special-registry/>
fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, d] = ip.octets();
            !(a == 0 ||
                a >= 224 ||
                ip.is_private() ||
                ip.is_loopback() ||
                ip.is_link_local() ||
                ip.is_documentation() ||
                (a == 100 && (64..=127).contains(&b)) ||
                (a == 198 && (18..=19).contains(&b)) ||
                (a == 192 && b == 0 && c == 0 && d != 9 && d != 10) ||
                (a == 192 && b == 88 && c == 99))
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            match segments {
                // Globally reachable exceptions within the IETF protocol assignments.
                [0x2001, 1, 0, 0, 0, 0, 0, 1..=3] |
                [0x2001, 3 | 0x20..=0x3f, ..] |
                [0x2001, 4, 0x112, ..] => true,
                // The well-known NAT64 prefix inherits the embedded IPv4 address's scope.
                [0x64, 0xff9b, 0, 0, 0, 0, a, b] => is_public_ip(
                    std::net::Ipv4Addr::from((u32::from(a) << 16) | u32::from(b)).into(),
                ),
                [0x2001, 0..=0x1ff | 0xdb8, ..] | [0x2002, ..] | [0x3fff, 0..=0xfff, ..] => false,
                // Public unicast allocations are within 2000::/3. This excludes local,
                // mapped, multicast, discard-only and reserved address space.
                [first, ..] => first & 0xe000 == 0x2000,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[tokio::test]
    #[ignore]
    async fn get_external_ip() {
        reth_tracing::init_test_tracing();
        let ip = external_ip().await;
        dbg!(ip);
    }

    #[tokio::test]
    #[ignore]
    async fn get_external_ip_interval() {
        reth_tracing::init_test_tracing();
        let mut interval = ResolveNatInterval::interval(Default::default(), Duration::from_secs(5));

        let ip = interval.tick().await;
        dbg!(ip);
        let ip = interval.tick().await;
        dbg!(ip);
    }

    #[tokio::test(start_paused = true)]
    async fn interval_preserves_pending_resolution() {
        let period = Duration::from_secs(5);
        let next_ip: IpAddr = "203.0.113.7".parse().unwrap();
        for result in [Some("203.0.113.8".parse().unwrap()), None] {
            let mut interval =
                ResolveNatInterval::interval(NatResolver::ExternalIp(next_ip), period);
            assert_eq!(interval.tick().await, Some(next_ip));

            let (tx, rx) = tokio::sync::oneshot::channel();
            interval.future = Some(Box::pin(async move { rx.await.unwrap() }));
            assert!(interval.tick().now_or_never().is_none());

            tokio::time::advance(period * 3).await;
            assert!(interval.tick().now_or_never().is_none());
            tx.send(result).expect("the pending resolution must not be dropped");
            assert_eq!(interval.tick().await, result);

            // The next attempt waits for its interval after either success or failure.
            assert!(interval.tick().now_or_never().is_none());
            tokio::time::advance(period).await;
            assert_eq!(interval.tick().await, Some(next_ip));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn interval_skips_missed_attempts() {
        let period = Duration::from_secs(5);
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        let mut interval = ResolveNatInterval::interval(NatResolver::ExternalIp(ip), period);
        assert_eq!(interval.tick().await, Some(ip));

        tokio::time::advance(period * 3).await;
        assert_eq!(interval.tick().await, Some(ip));
        assert!(interval.tick().now_or_never().is_none());
        tokio::time::advance(period).await;
        assert_eq!(interval.tick().await, Some(ip));
    }

    #[test]
    fn netif_resolution_does_not_block_the_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        // Occupy the blocking pool so a lookup must yield until the worker is released.
        let blocker = runtime.spawn_blocking(move || {
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        runtime.block_on(async {
            let mut lookup = std::pin::pin!(NatResolver::NetIf.external_addr());
            let pending = futures_util::poll!(&mut lookup).is_pending();
            release_tx.send(()).unwrap();
            assert!(pending, "network interface lookup must run on a blocking worker");
            assert_eq!(lookup.await, resolve_net_if_ip(DEFAULT_NET_IF_NAME).ok());
            blocker.await.unwrap();
        });
    }

    #[test]
    fn as_external_ip_test() {
        let resolver = NatResolver::ExternalAddr("localhost".to_string());
        let ip = resolver.as_external_ip(30303).expect("localhost should be resolvable");

        if ip.is_ipv4() {
            assert_eq!(ip, IpAddr::V4(Ipv4Addr::LOCALHOST));
        } else {
            assert_eq!(ip, IpAddr::V6(Ipv6Addr::LOCALHOST));
        }
    }

    #[test]
    fn test_from_str() {
        assert_eq!(NatResolver::Any, "any".parse().unwrap());
        assert_eq!(NatResolver::None, "none".parse().unwrap());

        let ip = NatResolver::ExternalIp(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let s = "extip:0.0.0.0";
        assert_eq!(ip, s.parse().unwrap());
        assert_eq!(ip.to_string(), s);
    }

    #[test]
    fn public_ip_scope() {
        for addr in [
            "0.0.0.0",
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "100.127.255.255",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "192.168.1.1",
            "192.0.0.8",
            "192.0.2.1",
            "192.88.99.2",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::ffff:8.8.8.8",
            "64:ff9b::a00:1",
            "64:ff9b:1::1",
            "100::1",
            "100:0:0:1::1",
            "2001::1",
            "2001:2::1",
            "2001:db8::1",
            "2002::1",
            "3fff::1",
            "3fff:fff:ffff::1",
            "5f00::1",
            "fc00::1",
            "fd00::1",
            "fe80::1",
            "fec0::1",
            "ff0e::1",
        ] {
            assert!(!is_public_ip(addr.parse().unwrap()), "{addr}");
        }
        for addr in [
            "1.1.1.1",
            "8.8.8.8",
            "100.63.255.255",
            "100.128.0.1",
            "192.0.0.9",
            "192.0.0.10",
            "198.17.255.255",
            "198.20.0.1",
            "64:ff9b::808:808",
            "2001:1::1",
            "2001:1::2",
            "2001:1::3",
            "2001:3::1",
            "2001:4:112::1",
            "2001:20::1",
            "2001:30::1",
            "2001:4860:4860::8888",
            "2606:4700:4700::1111",
            "3fff:1000::1",
        ] {
            assert!(is_public_ip(addr.parse().unwrap()), "{addr}");
        }
    }

    #[tokio::test]
    async fn provider_rejects_non_public_responses() {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for (body, expected) in [
            ("127.0.0.1", None),
            ("fd00::1", None),
            ("not an IP", None),
            (" 8.8.8.8\n", Some("8.8.8.8".parse().unwrap())),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 1024];
                let mut len = 0;
                while !request[..len].ends_with(b"\r\n\r\n") {
                    let read = socket.read(&mut request[len..]).await.unwrap();
                    assert!(read > 0);
                    len += read;
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            });
            assert_eq!(resolve_external_ip_url(&client, &url).await, expected);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn configured_private_addresses_remain_valid() {
        let ip = "10.0.0.1".parse().unwrap();
        assert_eq!(NatResolver::ExternalIp(ip).external_addr().await, Some(ip));
        assert_eq!(NatResolver::ExternalAddr("10.0.0.1".into()).external_addr().await, Some(ip));
    }
}
