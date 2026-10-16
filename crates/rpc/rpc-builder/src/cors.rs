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

/// Rejects WebSocket upgrades from origins outside the configured allowlist.
#[derive(Clone, Debug)]
pub(crate) struct WsOriginValidator {
    origins: Option<Vec<HeaderValue>>,
}

impl WsOriginValidator {
    pub(crate) fn new(domains: Option<&str>) -> Result<Self, CorsDomainError> {
        Ok(Self { origins: domains.map(parse_origins).transpose()?.flatten() })
    }
}

impl<B> ValidateRequest<B> for WsOriginValidator {
    type ResponseBody = HttpBody;

    fn validate(&mut self, request: &mut Request<B>) -> Result<(), Response<HttpBody>> {
        if let Some(allowed) = &self.origins &&
            ws::is_upgrade_request(request)
        {
            let mut origins = request.headers().get_all(ORIGIN).iter();
            // Originless clients are allowed; an explicit Origin must match exactly once.
            if let Some(origin) = origins.next() &&
                (origins.next().is_some() || !allowed.contains(origin))
            {
                let mut response = Response::new(HttpBody::empty());
                *response.status_mut() = StatusCode::FORBIDDEN;
                return Err(response)
            }
        }
        Ok(())
    }
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
