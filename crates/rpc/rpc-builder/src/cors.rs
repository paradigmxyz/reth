use hyper::{http::HeaderValue, Method};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

/// Error thrown when parsing cors domains went wrong
#[derive(Debug, thiserror::Error)]
pub(crate) enum CorsDomainError {
    #[error("{domain} is an invalid header value")]
    InvalidHeader { domain: String },
    #[error("Wildcard origin (`*`) cannot be passed as part of a list: {input}")]
    WildCardNotAllowed { input: String },
}

/// Creates a [CorsLayer] from the given domains
pub(crate) fn create_cors_layer(http_cors_domains: &str) -> Result<CorsLayer, CorsDomainError> {
    let cors = match http_cors_domains.trim() {
        "*" => CorsLayer::new()
            .allow_methods([Method::GET, Method::POST])
            .allow_origin(Any)
            .allow_headers(Any),
        _ => {
            let iter = http_cors_domains.split(',');
            if iter.clone().any(|o| o == "*") {
                return Err(CorsDomainError::WildCardNotAllowed {
                    input: http_cors_domains.to_string(),
                })
            }

            let origins = iter
                .map(|domain| {
                    domain
                        .parse::<HeaderValue>()
                        .map_err(|_| CorsDomainError::InvalidHeader { domain: domain.to_string() })
                })
                .collect::<Result<Vec<HeaderValue>, _>>()?;

            let origin = AllowOrigin::list(origins);
            CorsLayer::new()
                .allow_methods([Method::GET, Method::POST])
                .allow_origin(origin)
                .allow_headers(Any)
        }
    };
    Ok(cors)
}
