use http::{
    header::{HOST, ORIGIN},
    uri::Authority,
    HeaderValue, Method, Request, Response, StatusCode,
};
use jsonrpsee::server::{ws, HttpBody};
use std::{net::IpAddr, sync::Arc};
use tower_http::{
    cors::{AllowOrigin, Any, CorsLayer},
    validate_request::ValidateRequest,
};

/// Error thrown when parsing cors domains went wrong
#[derive(Debug, thiserror::Error)]
pub enum CorsDomainError {
    /// Represents an invalid header value for a domain
    #[error("{domain} is an invalid header value")]
    InvalidHeader {
        /// The domain that caused the invalid header
        domain: String,
    },

    /// Indicates that a wildcard origin was used incorrectly in a list
    #[error("wildcard origin (`*`) cannot be passed as part of a list: {input}")]
    WildCardNotAllowed {
        /// The input string containing the incorrectly used wildcard
        input: String,
    },
}

/// Creates a [`CorsLayer`] from the given domains
pub(crate) fn create_cors_layer(http_cors_domains: &str) -> Result<CorsLayer, CorsDomainError> {
    let cors = match parse_origins(http_cors_domains)? {
        None => CorsLayer::new()
            .allow_methods([Method::GET, Method::POST])
            .allow_origin(Any)
            .allow_headers(Any),
        Some(origins) => {
            let origin = AllowOrigin::list(origins);
            CorsLayer::new()
                .allow_methods([Method::GET, Method::POST])
                .allow_origin(origin)
                .allow_headers(Any)
        }
    };
    Ok(cors)
}

/// Origin policy for `WebSocket` upgrades.
#[derive(Clone, Debug)]
pub(crate) enum WsOriginValidator {
    Local,
    Any,
    List(Vec<HeaderValue>),
}

impl WsOriginValidator {
    pub(crate) fn new(domains: Option<&str>) -> Result<Self, CorsDomainError> {
        let Some(domains) = domains else { return Ok(Self::Local) };
        Ok(match parse_origins(domains)? {
            Some(origins) => Self::List(origins),
            None => Self::Any,
        })
    }
}

impl<B> ValidateRequest<B> for WsOriginValidator {
    type ResponseBody = HttpBody;

    fn validate(&mut self, request: &mut Request<B>) -> Result<(), Response<HttpBody>> {
        if ws::is_upgrade_request(request) {
            let mut origins = request.headers().get_all(ORIGIN).iter();
            // Native clients can omit Origin.
            let Some(origin) = origins.next() else { return Ok(()) };
            let allowed = match self {
                Self::Any => return Ok(()),
                Self::Local => is_local_origin(origin),
                Self::List(allowed) => allowed.contains(origin),
            };
            if !allowed || origins.next().is_some() {
                let mut response = Response::new(HttpBody::empty());
                *response.status_mut() = StatusCode::FORBIDDEN;
                return Err(response)
            }
        }
        Ok(())
    }
}

/// `Host` policy for ordinary HTTP requests.
///
/// Requests whose `Host` is not allowed are rejected before dispatch. IP literals are always
/// accepted.
#[derive(Clone, Debug)]
pub(crate) enum HostValidator {
    /// Accepts `localhost`.
    Local,
    /// Accepts only the listed names, case-insensitively. Shared because the server clones the
    /// validator for every connection and request.
    List(Arc<[String]>),
}

impl HostValidator {
    /// Returns `None` when `hosts` contains `*`, which accepts any host.
    pub(crate) fn new(hosts: Option<&str>) -> Option<Self> {
        let Some(hosts) = hosts else { return Some(Self::Local) };
        let hosts = hosts.split(',').map(str::trim);
        if hosts.clone().any(|host| host == "*") {
            return None
        }
        // Ports are ignored on both sides.
        Some(Self::List(
            hosts
                .map(|host| {
                    host.parse::<Authority>()
                        .map_or_else(|_| host.to_string(), |a| a.host().to_string())
                })
                .collect(),
        ))
    }

    fn allows(&self, authority: &Authority) -> bool {
        let host = authority.host();
        // `host()` drops userinfo and anything after a bracketed literal, so reject those.
        if !authority
            .as_str()
            .strip_prefix(host)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
        {
            return false
        }
        let ip = host.strip_prefix('[').and_then(|host| host.strip_suffix(']')).unwrap_or(host);
        if ip.parse::<IpAddr>().is_ok() {
            return true
        }
        match self {
            Self::Local => host.eq_ignore_ascii_case("localhost"),
            Self::List(allowed) => allowed.iter().any(|allowed| allowed.eq_ignore_ascii_case(host)),
        }
    }
}

impl<B> ValidateRequest<B> for HostValidator {
    type ResponseBody = HttpBody;

    fn validate(&mut self, request: &mut Request<B>) -> Result<(), Response<HttpBody>> {
        // `WsOriginValidator` owns the policy for upgrades, which are always `GET`.
        if request.method() == Method::GET && ws::is_upgrade_request(request) {
            return Ok(())
        }
        let mut hosts = request.headers().get_all(HOST).iter();
        let host = hosts.next();
        // HTTP/2 and absolute-form requests also carry the host in the URI authority.
        let allowed = hosts.next().is_none() &&
            host.is_none_or(|host| {
                host.is_empty() ||
                    Authority::try_from(host.as_bytes())
                        .is_ok_and(|authority| self.allows(&authority))
            }) &&
            request.uri().authority().is_none_or(|authority| self.allows(authority));
        if allowed {
            return Ok(())
        }
        let mut response = Response::new(HttpBody::from("invalid host specified"));
        *response.status_mut() = StatusCode::FORBIDDEN;
        Err(response)
    }
}

fn is_local_origin(origin: &HeaderValue) -> bool {
    let Ok(origin) = origin.to_str() else { return false };
    let Some(("http" | "https", authority)) = origin.split_once("://") else { return false };
    // Browser origins contain only a scheme, host and optional port.
    let host = match authority.rsplit_once(':') {
        Some((host, port)) if !port.starts_with('+') && port.parse::<u16>().is_ok() => host,
        _ => authority,
    };
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

fn parse_origins(domains: &str) -> Result<Option<Vec<HeaderValue>>, CorsDomainError> {
    if domains.trim() == "*" {
        return Ok(None)
    }
    let iter = domains.split(',').map(str::trim);
    if iter.clone().any(|origin| origin == "*") {
        return Err(CorsDomainError::WildCardNotAllowed { input: domains.to_string() })
    }
    iter.map(|domain| {
        domain.parse().map_err(|_| CorsDomainError::InvalidHeader { domain: domain.to_string() })
    })
    .collect::<Result<Vec<_>, _>>()
    .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wildcard_with_spaces_rejected() {
        // Wildcard with leading space should still be rejected
        let result = create_cors_layer("http://example.com, *");
        assert!(matches!(result, Err(CorsDomainError::WildCardNotAllowed { .. })));
    }

    #[test]
    fn test_host_validator() {
        let mut local = HostValidator::new(None).unwrap();
        let list = HostValidator::new(Some("rpc.example.com:8545, other.example")).unwrap();
        for (hosts, local_allowed, list_allowed) in [
            (vec![], true, true),
            (vec![""], true, true),
            (vec!["localhost:8545"], true, false),
            (vec!["LOCALHOST"], true, false),
            (vec!["reth:8545"], false, false),
            (vec!["127.0.0.1:8545"], true, true),
            (vec!["[::1]:8545"], true, true),
            (vec!["10.0.0.1"], true, true),
            (vec!["RPC.example.com:443"], false, true),
            (vec!["unknown.example:8545"], false, false),
            (vec!["localhost."], false, false),
            (vec!["127.0.0.1.nip.io"], false, false),
            (vec!["0x7f.1"], false, false),
            (vec!["bad host"], false, false),
            (vec!["[::1].unknown.example"], false, false),
            (vec!["unknown.example@127.0.0.1"], false, false),
            (vec!["localhost", "localhost"], false, false),
        ] {
            for (validator, allowed) in [(&local, local_allowed), (&list, list_allowed)] {
                let mut request = Request::new(());
                for host in &hosts {
                    request.headers_mut().append(HOST, HeaderValue::from_static(host));
                }
                let status = validator.clone().validate(&mut request).err().map(|r| r.status());
                assert_eq!(
                    status,
                    (!allowed).then_some(StatusCode::FORBIDDEN),
                    "{validator:?}, hosts={hosts:?}"
                );
            }
        }

        for (uri, host, allowed) in [
            ("http://localhost:8545/", None, true),
            ("http://unknown.example/", None, false),
            ("http://unknown.example/", Some("localhost"), false),
            ("http://unknown.example/", Some(""), false),
        ] {
            let mut request = Request::builder().uri(uri).body(()).unwrap();
            if let Some(host) = host {
                request.headers_mut().insert(HOST, HeaderValue::from_static(host));
            }
            assert_eq!(local.validate(&mut request).is_ok(), allowed, "{uri}, host={host:?}");
        }

        // Upgrades are left to `WsOriginValidator`.
        for (method, allowed) in [(Method::GET, true), (Method::OPTIONS, false)] {
            let mut request = Request::builder()
                .method(&method)
                .header(HOST, "unknown.example")
                .header("connection", "Upgrade")
                .header("upgrade", "websocket")
                .body(())
                .unwrap();
            assert_eq!(local.validate(&mut request).is_ok(), allowed, "{method}");
        }

        assert!(HostValidator::new(Some("rpc.example.com, *")).is_none());
    }
}
