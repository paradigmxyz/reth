// Copyright 2019-2023 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! HTTP host validation middleware.

use crate::middleware::http::authority::{Authority, AuthorityError, Port};
use crate::transport::http;
use crate::{HttpBody, HttpRequest, LOG_TARGET};
use futures_util::{Future, FutureExt, TryFutureExt};
use hyper::Response;
use hyper::body::Bytes;
use jsonrpsee_core::BoxError;
use route_recognizer::Router;
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{Layer, Service};

type Ports = Vec<Port>;

/// Middleware to enable host filtering.
#[derive(Debug, Clone)]
pub struct HostFilterLayer(Option<Arc<WhitelistedHosts>>);

impl HostFilterLayer {
	/// Enables host filtering and allow only the specified hosts.
	pub fn new<T, U>(allow_only: T) -> Result<Self, AuthorityError>
	where
		T: IntoIterator<Item = U>,
		U: TryInto<Authority, Error = AuthorityError>,
	{
		let allow_only: Result<Vec<_>, _> = allow_only.into_iter().map(|a| a.try_into()).collect();
		Ok(Self(Some(Arc::new(WhitelistedHosts::from(allow_only?)))))
	}

	/// Convenience method to disable host filtering but less efficient
	/// than to not enable the middleware at all.
	///
	/// Because is the `tower middleware` returns a different type
	/// depending on which Layers are configured it and may not compile
	/// in some contexts.
	///
	/// For example the following won't compile:
	///
	/// ```ignore
	/// use jsonrpsee_server::middleware::{ProxyGetRequestLayer, HostFilterLayer};
	///
	/// let host_filter = false;
	///
	/// let middleware = if host_filter {
	///     tower::ServiceBuilder::new()
	///        .layer(HostFilterLayer::new(["example.com"]).unwrap())
	///        .layer(ProxyGetRequestLayer::new("/health", "system_health").unwrap())
	/// } else {
	///    tower::ServiceBuilder::new()
	/// };
	/// ```
	pub fn disable() -> Self {
		Self(None)
	}
}

impl<S> Layer<S> for HostFilterLayer {
	type Service = HostFilter<S>;

	fn layer(&self, inner: S) -> Self::Service {
		HostFilter { inner, filter: self.0.clone() }
	}
}

/// Middleware to enable host filtering.
#[derive(Debug, Clone)]
pub struct HostFilter<S> {
	inner: S,
	filter: Option<Arc<WhitelistedHosts>>,
}

impl<S, B> Service<HttpRequest<B>> for HostFilter<S>
where
	S: Service<HttpRequest<B>, Response = Response<HttpBody>>,
	S::Response: 'static,
	S::Error: Into<BoxError> + 'static,
	S::Future: Send + 'static,
	B: http_body::Body<Data = Bytes> + Send + std::fmt::Debug + 'static,
	B::Data: Send,
	B::Error: Into<BoxError>,
{
	type Response = S::Response;
	type Error = BoxError;
	type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

	fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		self.inner.poll_ready(cx).map_err(Into::into)
	}

	fn call(&mut self, request: HttpRequest<B>) -> Self::Future {
		let Some(authority) = Authority::from_http_request(&request) else {
			return async { Ok(http::response::malformed()) }.boxed();
		};

		if self.filter.as_ref().is_none_or(|f| f.recognize(&authority)) {
			Box::pin(self.inner.call(request).map_err(Into::into))
		} else {
			tracing::debug!(target: LOG_TARGET, "Denied request: {:?}", request);
			async { Ok(http::response::host_not_allowed()) }.boxed()
		}
	}
}

/// Represent the URL patterns that is whitelisted.
#[derive(Default, Debug, Clone)]
pub struct WhitelistedHosts(Router<Ports>);

impl<T> From<T> for WhitelistedHosts
where
	T: IntoIterator<Item = Authority>,
{
	fn from(value: T) -> Self {
		let mut router = Router::new();
		let mut uniq_hosts: BTreeMap<String, Ports> = BTreeMap::new();

		// Ensure that no ports are "overwritten"
		// since it's possible add the same hostname with
		// several port numbers.
		for auth in value.into_iter() {
			uniq_hosts
				.entry(auth.host)
				.and_modify(|v| {
					v.push(auth.port);
				})
				.or_insert_with(|| vec![auth.port]);
		}

		for (host, ports) in uniq_hosts.into_iter() {
			router.add(&host, ports);
		}

		Self(router)
	}
}

impl WhitelistedHosts {
	fn recognize(&self, other: &Authority) -> bool {
		if let Ok(p) = self.0.recognize(&other.host) {
			let ports = p.handler();

			ports.iter().any(|p| match (p, &other.port) {
				(Port::Any, _) => true,
				(Port::Default, Port::Default) => true,
				(Port::Fixed(p1), Port::Fixed(p2)) if p1 == p2 => true,
				_ => false,
			})
		} else {
			false
		}
	}
}

#[cfg(test)]
mod tests {
	use super::{Authority, WhitelistedHosts};

	fn unwrap_auth(a: &str) -> Authority {
		a.try_into().unwrap()
	}

	fn unwrap_filter(list: &[&str]) -> WhitelistedHosts {
		let l: Vec<_> = list.iter().map(|&a| a.try_into().unwrap()).collect();
		WhitelistedHosts::from(l)
	}

	#[test]
	fn should_reject_if_header_not_on_the_list() {
		let filter = unwrap_filter(&[]);
		assert!(!filter.recognize(&unwrap_auth("parity.io")));
	}

	#[test]
	fn should_accept_if_on_the_list() {
		let filter = unwrap_filter(&["parity.io"]);
		assert!(filter.recognize(&unwrap_auth("parity.io")));
	}

	#[test]
	fn should_accept_if_on_the_list_with_port() {
		let filter = unwrap_filter(&["parity.io:443", "parity.io:9944"]);
		assert!(filter.recognize(&unwrap_auth("parity.io:443")));
		assert!(filter.recognize(&unwrap_auth("parity.io:9944")));
		assert!(!filter.recognize(&unwrap_auth("parity.io")));
	}

	#[test]
	fn should_support_wildcards() {
		let filter = unwrap_filter(&["*.web3.site:*"]);
		assert!(filter.recognize(&unwrap_auth("parity.web3.site:8180")));
		assert!(filter.recognize(&unwrap_auth("parity.web3.site")));
	}

	#[test]
	fn should_accept_with_and_without_default_port() {
		let filter = unwrap_filter(&["https://parity.io:443"]);
		assert!(filter.recognize(&unwrap_auth("https://parity.io")));
		assert!(filter.recognize(&unwrap_auth("https://parity.io:443")));
	}
}
