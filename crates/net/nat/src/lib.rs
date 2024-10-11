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
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub mod net_if;

pub use net_if::{NetInterfaceError, DEFAULT_NET_IF_NAME};

use std::{
    fmt,
    future::{poll_fn, Future},
    net::{AddrParseError, IpAddr},
    pin::Pin,
    str::FromStr,
    task::{Context, Poll},
    time::Duration,
};
use tracing::{debug, error};

use crate::net_if::resolve_net_if_ip;
#[cfg(feature = "serde")]
use serde_with::{DeserializeFromStr, SerializeDisplay};

/// URLs to `GET` the external IP address.
///
/// Taken from: <https://stackoverflow.com/questions/3253701/get-public-external-ip-address>
const EXTERNAL_IP_APIS: &[&str] =
    &["http://ipinfo.io/ip", "http://icanhazip.com", "http://ifconfig.me"];

/// All builtin resolvers.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default, Hash)]
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
    /// Resolve external IP via the network interface.
    NetIf,
    /// Resolve nothing
    None,
}

impl NatResolver {
    /// Attempts to produce an IP address (best effort).
    pub async fn external_addr(self) -> Option<IpAddr> {
        external_addr_with(self).await
    }

    /// Returns the external ip, if it is [`NatResolver::ExternalIp`]
    pub const fn as_external_ip(self) -> Option<IpAddr> {
        match self {
            Self::ExternalIp(ip) => Some(ip),
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
                let Some(ip) = s.strip_prefix("extip:") else {
                    return Err(ParseNatResolverError::UnknownVariant(format!(
                        "Unknown Nat Resolver: {s}"
                    )))
                };
                Self::ExternalIp(ip.parse::<IpAddr>()?)
            }
        };
        Ok(r)
    }
}

/// With this type you can resolve the external public IP address on an interval basis.
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
    fn with_interval(resolver: NatResolver, interval: tokio::time::Interval) -> Self {
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
        if self.interval.poll_tick(cx).is_ready() {
            self.future = Some(Box::pin(self.resolver.external_addr()));
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
pub async fn external_ip() -> Option<IpAddr> {
    external_addr_with(NatResolver::Any).await
}

/// Given a [`NatResolver`] attempts to produce an IP address (best effort).
pub async fn external_addr_with(resolver: NatResolver) -> Option<IpAddr> {
    match resolver {
        NatResolver::Any | NatResolver::Upnp | NatResolver::PublicIp => resolve_external_ip().await,
        NatResolver::ExternalIp(ip) => Some(ip),
        NatResolver::NetIf => resolve_net_if_ip(DEFAULT_NET_IF_NAME)
            .inspect_err(|err| {
                debug!(target: "net::nat",
                     %err,
                    "Failed to resolve network interface IP"
                );
            })
            .ok(),
        NatResolver::None => None,
    }
}

async fn resolve_external_ip() -> Option<IpAddr> {
    let futures = EXTERNAL_IP_APIS.iter().copied().map(resolve_external_ip_url_res).map(Box::pin);
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

async fn resolve_external_ip_url_res(url: &str) -> Result<IpAddr, ()> {
    resolve_external_ip_url(url).await.ok_or(())
}

async fn resolve_external_ip_url(url: &str) -> Option<IpAddr> {
    let response = reqwest::get(url).await.ok()?;
    let response = response.error_for_status().ok()?;
    let text = response.text().await.ok()?;
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

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

    #[test]
    fn test_from_str() {
        assert_eq!(NatResolver::Any, "any".parse().unwrap());
        assert_eq!(NatResolver::None, "none".parse().unwrap());

        let ip = NatResolver::ExternalIp(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let s = "extip:0.0.0.0";
        assert_eq!(ip, s.parse().unwrap());
        assert_eq!(ip.to_string().as_str(), s);
    }
}
