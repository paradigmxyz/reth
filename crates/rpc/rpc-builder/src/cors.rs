use http::{header::ORIGIN, HeaderValue, Method, Request, Response, StatusCode};
use jsonrpsee::server::{ws, HttpBody};
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
}
