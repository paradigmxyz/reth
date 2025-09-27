use http::{HeaderValue, Method};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

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
    let cors = match http_cors_domains.trim() {
        "*" => CorsLayer::new()
            .allow_methods([Method::GET, Method::POST])
            .allow_origin(Any)
            .allow_headers(Any),
        list => {
            // Normalize the comma-separated list: trim each token and drop empties
            // This makes common inputs like "https://a.com, https://b.com" robust.
            let items: Vec<&str> = list
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();

            // After normalization, wildcard is not allowed as part of a list
            if items.iter().any(|o| *o == "*") {
                return Err(CorsDomainError::WildCardNotAllowed {
                    input: http_cors_domains.to_string(),
                    })
            }

            let origins = items
                .into_iter()
                .map(|domain| {
                    HeaderValue::from_str(domain)
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
