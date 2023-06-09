#![warn(missing_docs, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Helpers for resolving the external IP.

use igd::aio::search_gateway;
use pin_project_lite::pin_project;
use std::{
    fmt,
    future::{poll_fn, Future},
    net::{AddrParseError, IpAddr},
    pin::Pin,
    str::FromStr,
    task::{ready, Context, Poll},
    time::Duration,
};
use tracing::debug;

#[cfg(feature = "serde")]
use serde_with::{DeserializeFromStr, SerializeDisplay};

/// All builtin resolvers.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default, Hash)]
#[cfg_attr(feature = "serde", derive(SerializeDisplay, DeserializeFromStr))]
pub enum NatResolver {
    /// Resolve with any available resolver.
    #[default]
    Any,
    /// Resolve via Upnp
    Upnp,
    /// Resolve external IP via [public_ip::Resolver]
    PublicIp,
    /// Use the given [IpAddr]
    ExternalIp(IpAddr),
    /// Resolve nothing
    None,
}

// === impl NatResolver ===

impl NatResolver {
    /// Attempts to produce an IP address (best effort).
    pub async fn external_addr(self) -> Option<IpAddr> {
        external_addr_with(self).await
    }
}

impl fmt::Display for NatResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NatResolver::Any => f.write_str("any"),
            NatResolver::Upnp => f.write_str("upnp"),
            NatResolver::PublicIp => f.write_str("publicip"),
            NatResolver::ExternalIp(ip) => write!(f, "extip:{ip}"),
            NatResolver::None => f.write_str("none"),
        }
    }
}

/// Error when parsing a [NatResolver]
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
            "any" => NatResolver::Any,
            "upnp" => NatResolver::Upnp,
            "none" => NatResolver::None,
            "publicip" | "public-ip" => NatResolver::PublicIp,
            s => {
                let Some(ip) = s.strip_prefix("extip:") else { return Err(ParseNatResolverError::UnknownVariant(format!(
                        "Unknown Nat Resolver: {s}"
                    ))) };
                NatResolver::ExternalIp(ip.parse::<IpAddr>()?)
            }
        };
        Ok(r)
    }
}

/// With this type you can resolve the external public IP address on an interval basis.
#[must_use = "Does nothing unless polled"]
pub struct ResolveNatInterval {
    resolver: NatResolver,
    future: Option<ResolveFut>,
    interval: tokio::time::Interval,
}

// === impl ResolveNatInterval ===

impl ResolveNatInterval {
    fn with_interval(resolver: NatResolver, interval: tokio::time::Interval) -> Self {
        Self { resolver, future: None, interval }
    }

    /// Creates a new [ResolveNatInterval] that attempts to resolve the public IP with interval of
    /// period. See also [tokio::time::interval]
    #[track_caller]
    pub fn interval(resolver: NatResolver, period: Duration) -> Self {
        let interval = tokio::time::interval(period);
        Self::with_interval(resolver, interval)
    }

    /// Creates a new [ResolveNatInterval] that attempts to resolve the public IP with interval of
    /// period with the first attempt starting at `sart`. See also [tokio::time::interval_at]
    #[track_caller]
    pub fn interval_at(
        resolver: NatResolver,
        start: tokio::time::Instant,
        period: Duration,
    ) -> Self {
        let interval = tokio::time::interval_at(start, period);
        Self::with_interval(resolver, interval)
    }

    /// Completes when the next [IpAddr] in the interval has been reached.
    pub async fn tick(&mut self) -> Option<IpAddr> {
        let ip = poll_fn(|cx| self.poll_tick(cx));
        ip.await
    }

    /// Polls for the next resolved [IpAddr] in the interval to be reached.
    ///
    /// This method can return the following values:
    ///
    ///  * `Poll::Pending` if the next [IpAddr] has not yet been resolved.
    ///  * `Poll::Ready(Option<IpAddr>)` if the next [IpAddr] has been resolved. This returns `None`
    ///    if the attempt was unsuccessful.
    pub fn poll_tick(&mut self, cx: &mut Context<'_>) -> Poll<Option<IpAddr>> {
        if self.interval.poll_tick(cx).is_ready() {
            self.future = Some(Box::pin(self.resolver.external_addr()));
        }

        if let Some(mut fut) = self.future.take() {
            match fut.as_mut().poll(cx) {
                Poll::Ready(ip) => return Poll::Ready(ip),
                Poll::Pending => {
                    self.future = Some(fut);
                }
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
        NatResolver::Any => {
            ResolveAny {
                upnp: Some(Box::pin(resolve_external_ip_upnp())),
                external: Some(Box::pin(resolve_external_ip())),
            }
            .await
        }
        NatResolver::Upnp => resolve_external_ip_upnp().await,
        NatResolver::PublicIp => resolve_external_ip().await,
        NatResolver::ExternalIp(ip) => Some(ip),
        NatResolver::None => None,
    }
}

type ResolveFut = Pin<Box<dyn Future<Output = Option<IpAddr>> + Send>>;

pin_project! {
    /// A future that resolves the first ip via all configured resolvers
    struct ResolveAny {
        #[pin]
        upnp: Option<ResolveFut>,
         #[pin]
        external: Option<ResolveFut>,
    }
}

impl Future for ResolveAny {
    type Output = Option<IpAddr>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.as_mut().project();

        if let Some(upnp) = this.upnp.as_mut().as_pin_mut() {
            // if upnp is configured we prefer it over http and dns resolvers
            let ip = ready!(upnp.poll(cx));
            this.upnp.set(None);
            if ip.is_some() {
                return Poll::Ready(ip)
            }
        }

        if let Some(upnp) = this.external.as_mut().as_pin_mut() {
            if let Poll::Ready(ip) = upnp.poll(cx) {
                this.external.set(None);
                if ip.is_some() {
                    return Poll::Ready(ip)
                }
            }
        }

        if this.upnp.is_none() && this.external.is_none() {
            return Poll::Ready(None)
        }

        Poll::Pending
    }
}

async fn resolve_external_ip_upnp() -> Option<IpAddr> {
    search_gateway(Default::default())
        .await
        .map_err(|err| {
            debug!(target: "net::nat", ?err, "Failed to resolve external IP via UPnP: failed to find gateway");
            err
        })
        .ok()?
        .get_external_ip()
        .await
        .map_err(|err| {
            debug!(target: "net::nat", ?err, "Failed to resolve external IP via UPnP");
            err
        })
        .ok()
}

async fn resolve_external_ip() -> Option<IpAddr> {
    public_ip::addr().await
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
