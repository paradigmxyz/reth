use crate::{AuthValidator, JwtError, JwtSecret};
use http::{header, HeaderMap, Response, StatusCode};
use jsonrpsee_http_client::{HttpBody, HttpResponse};
use tracing::error;

/// Implements JWT validation logics and integrates
/// to an Http [`AuthLayer`][crate::AuthLayer]
/// by implementing the [`AuthValidator`] trait.
#[derive(Debug, Clone)]
pub struct JwtAuthValidator {
    secret: JwtSecret,
}

impl JwtAuthValidator {
    /// Creates a new instance of [`JwtAuthValidator`].
    /// Validation logics are implemented by the `secret`
    /// argument (see [`JwtSecret`]).
    pub const fn new(secret: JwtSecret) -> Self {
        Self { secret }
    }
}

impl AuthValidator for JwtAuthValidator {
    fn validate(&self, headers: &HeaderMap) -> Result<(), HttpResponse> {
        match get_bearer(headers) {
            Some(jwt) => match self.secret.validate(jwt) {
                Ok(_) => Ok(()),
                Err(e) => {
                    error!(target: "engine::jwt-validator", "Invalid JWT: {e}");
                    let response = err_response(e);
                    Err(response)
                }
            },
            None => {
                let e = JwtError::MissingOrInvalidAuthorizationHeader;
                error!(target: "engine::jwt-validator", "Invalid JWT: {e}");
                let response = err_response(e);
                Err(response)
            }
        }
    }
}

/// This is an utility function that retrieves a bearer
/// token from the Authorization header.
fn get_bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ")
}

fn err_response(err: JwtError) -> HttpResponse {
    // We build a response from an error message.
    // We don't cope with headers or other structured fields.
    // Then we are safe to "expect" on the result.
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .body(HttpBody::new(err.to_string()))
        .expect("This should never happen")
}

#[cfg(test)]
mod tests {
    use crate::jwt_validator::get_bearer;
    use http::{header, HeaderMap};

    #[test]
    fn auth_header_available() {
        let jwt = "foo";
        let bearer = format!("Bearer {jwt}");
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, bearer.parse().unwrap());
        let token = get_bearer(&headers).unwrap();
        assert_eq!(token, jwt);
        assert_eq!(
            token.as_ptr(),
            headers[header::AUTHORIZATION].as_bytes()[bearer.len() - jwt.len()..].as_ptr()
        );
    }

    #[test]
    fn auth_header_not_available() {
        let headers = HeaderMap::new();
        let token = get_bearer(&headers);
        assert!(token.is_none());
    }

    #[test]
    fn auth_header_malformed() {
        let jwt = "foo";
        let bearer = format!("Bea___rer {jwt}");
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, bearer.parse().unwrap());
        let token = get_bearer(&headers);
        assert!(token.is_none());
    }

    #[test]
    fn auth_header_bearer_in_middle() {
        // Test that "Bearer " must be at the start of the header, not in the middle
        let jwt = "valid_token";
        let bearer = format!("NotBearer Bearer {jwt}");
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, bearer.parse().unwrap());
        let token = get_bearer(&headers);
        // Function should return None since "Bearer " is not at the start
        assert!(token.is_none());
    }

    #[test]
    fn auth_header_bearer_without_space() {
        // Test that "BearerBearer" is not treated as "Bearer "
        let jwt = "valid_token";
        let bearer = format!("BearerBearer {jwt}");
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, bearer.parse().unwrap());
        let token = get_bearer(&headers);
        // Function should return None since header doesn't start with "Bearer "
        assert!(token.is_none());
    }
}
