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

//! Utility and types related to the authority of an URI.

use crate::HttpRequest;
use http::uri::{InvalidUri, Uri};
use jsonrpsee_core::http_helpers;

/// Represent the http URI scheme that is returned by the HTTP host header
///
/// Further information can be found: <https://www.rfc-editor.org/rfc/rfc7230#section-2.7.1>
#[derive(Clone, Hash, PartialEq, Eq, Debug)]
pub struct Authority {
	/// The host.
	pub host: String,
	/// The port.
	pub port: Port,
}

/// Error that can happen when parsing an URI authority fails.
#[derive(Debug, thiserror::Error)]
pub enum AuthorityError {
	/// Invalid URI.
	#[error(transparent)]
	InvalidUri(InvalidUri),
	/// Invalid port.
	#[error("Invalid port: {0}")]
	InvalidPort(String),
	/// The host was not found.
	#[error("The host was not found")]
	MissingHost,
}

/// Port pattern
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub enum Port {
	/// No port specified (default port)
	Default,
	/// Port specified as a wildcard pattern (*).
	Any,
	/// Fixed numeric port
	Fixed(u16),
}

impl Authority {
	fn inner_from_str(value: &str) -> Result<Self, AuthorityError> {
		let uri: Uri = value.parse().map_err(AuthorityError::InvalidUri)?;
		let authority = uri.authority().ok_or(AuthorityError::MissingHost)?;
		let host = authority.host();
		let maybe_port = &authority.as_str()[host.len()..];

		// After the host segment, the authority may contain a port such as `fooo:33`, `foo:*` or `foo`
		let port = match maybe_port.split_once(':') {
			Some((_, "*")) => Port::Any,
			Some((_, p)) => {
				let port_u16: u16 =
					p.parse().map_err(|e: std::num::ParseIntError| AuthorityError::InvalidPort(e.to_string()))?;

				// Omit default port to allow both requests with and without the default port.
				match default_port(uri.scheme_str()) {
					Some(p) if p == port_u16 => Port::Default,
					_ => port_u16.into(),
				}
			}
			None => Port::Default,
		};

		Ok(Self { host: host.to_owned(), port })
	}

	/// Attempts to parse the authority from a HTTP request.
	///
	/// The `Authority` can be sent by the client in the `Host header` or in the `URI`
	/// such that both must be checked.
	pub fn from_http_request<T>(request: &HttpRequest<T>) -> Option<Self> {
		// NOTE: we use our own `Authority type` here because an invalid port number would return `None` here
		// and that should be denied.
		let host_header =
			http_helpers::read_header_value(request.headers(), hyper::header::HOST).map(Authority::try_from);
		let uri = request.uri().authority().map(|v| Authority::try_from(v.as_str()));

		match (host_header, uri) {
			(Some(Ok(a1)), Some(Ok(a2))) => {
				if a1 == a2 {
					Some(a1)
				} else {
					None
				}
			}
			(Some(Ok(a)), _) => Some(a),
			(_, Some(Ok(a))) => Some(a),
			_ => None,
		}
	}
}

impl<'a> TryFrom<&'a str> for Authority {
	type Error = AuthorityError;

	fn try_from(value: &'a str) -> Result<Self, Self::Error> {
		Self::inner_from_str(value)
	}
}

impl TryFrom<String> for Authority {
	type Error = AuthorityError;

	fn try_from(value: String) -> Result<Self, Self::Error> {
		Self::inner_from_str(&value)
	}
}

impl TryFrom<std::net::SocketAddr> for Authority {
	type Error = AuthorityError;

	fn try_from(sockaddr: std::net::SocketAddr) -> Result<Self, Self::Error> {
		Self::inner_from_str(&sockaddr.to_string())
	}
}

impl From<u16> for Port {
	fn from(port: u16) -> Port {
		Port::Fixed(port)
	}
}

fn default_port(scheme: Option<&str>) -> Option<u16> {
	match scheme {
		Some("http") | Some("ws") => Some(80),
		Some("https") | Some("wss") => Some(443),
		Some("ftp") => Some(21),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::{Authority, HttpRequest, Port};
	use hyper::header::HOST;

	fn authority(host: &str, port: Port) -> Authority {
		Authority { host: host.to_owned(), port }
	}

	type EmptyBody = http_body_util::Empty<hyper::body::Bytes>;

	#[test]
	fn should_parse_valid_authority() {
		assert_eq!(Authority::try_from("http://parity.io").unwrap(), authority("parity.io", Port::Default));
		assert_eq!(Authority::try_from("https://parity.io:8443").unwrap(), authority("parity.io", Port::Fixed(8443)));
		assert_eq!(Authority::try_from("chrome-extension://124.0.0.1").unwrap(), authority("124.0.0.1", Port::Default));
		assert_eq!(Authority::try_from("http://*.domain:*/somepath").unwrap(), authority("*.domain", Port::Any));
		assert_eq!(Authority::try_from("parity.io").unwrap(), authority("parity.io", Port::Default));
		assert_eq!(Authority::try_from("127.0.0.1:8845").unwrap(), authority("127.0.0.1", Port::Fixed(8845)));
		assert_eq!(
			Authority::try_from("http://[2001:db8:85a3:8d3:1319:8a2e:370:7348]:9933/").unwrap(),
			authority("[2001:db8:85a3:8d3:1319:8a2e:370:7348]", Port::Fixed(9933))
		);
		assert_eq!(
			Authority::try_from("http://[2001:db8:85a3:8d3:1319:8a2e:370:7348]/").unwrap(),
			authority("[2001:db8:85a3:8d3:1319:8a2e:370:7348]", Port::Default)
		);
		assert_eq!(
			Authority::try_from("https://user:password@example.com/tmp/foo").unwrap(),
			authority("example.com", Port::Default)
		);
	}

	#[test]
	fn should_not_parse_invalid_authority() {
		assert!(Authority::try_from("/foo/bar").is_err());
		assert!(Authority::try_from("user:password").is_err());
		assert!(Authority::try_from("parity.io/somepath").is_err());
		assert!(Authority::try_from("127.0.0.1:8545/somepath").is_err());
		assert!(Authority::try_from("127.0.0.1:-1337").is_err());
	}

	#[test]
	fn authority_from_http_only_host_works() {
		let req = HttpRequest::builder().header(HOST, "example.com").body(EmptyBody::new()).unwrap();
		assert!(Authority::from_http_request(&req).is_some());
	}

	#[test]
	fn authority_only_uri_works() {
		let req = HttpRequest::builder().uri("example.com").body(EmptyBody::new()).unwrap();
		assert!(Authority::from_http_request(&req).is_some());
	}

	#[test]
	fn authority_host_and_uri_works() {
		let req = HttpRequest::builder()
			.header(HOST, "example.com:9999")
			.uri("example.com:9999")
			.body(EmptyBody::new())
			.unwrap();
		assert!(Authority::from_http_request(&req).is_some());
	}

	#[test]
	fn authority_host_and_uri_mismatch() {
		let req =
			HttpRequest::builder().header(HOST, "example.com:9999").uri("example.com").body(EmptyBody::new()).unwrap();
		assert!(Authority::from_http_request(&req).is_none());
	}

	#[test]
	fn authority_missing_host_and_uri() {
		let req = HttpRequest::builder().body(EmptyBody::new()).unwrap();
		assert!(Authority::from_http_request(&req).is_none());
	}
}
