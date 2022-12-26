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
    future::Future,
    net::IpAddr,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tracing::warn;

/// All builtin resolvers.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default, Hash)]
pub enum NatResolver {
    /// Resolve with any available resolver.
    #[default]
    Any,
    /// Resolve via Upnp
    Upnp,
    /// Resolve external IP via [public_ip::Resolver]
    ExternalIp,
}

// === impl NatResolver ===

impl NatResolver {
    /// Attempts to produce an IP address (best effort).
    pub async fn external_addr(self) -> Option<IpAddr> {
        external_addr_with(self).await
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
        NatResolver::ExternalIp => resolve_external_ip().await,
    }
}

type ResolveFut = Pin<Box<dyn Future<Output = Option<IpAddr>>>>;

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
            warn!(target: "net::nat", ?err, "failed to find upnp gateway");
            err
        })
        .ok()?
        .get_external_ip()
        .await
        .map_err(|err| {
            warn!(target: "net::nat", ?err, "failed to resolve external ip via upnp gateway");
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

    #[tokio::test]
    #[ignore]
    async fn get_external_ip() {
        reth_tracing::init_tracing();
        let ip = external_ip().await;
        dbg!(ip);
    }
}
