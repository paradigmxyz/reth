use http::{HeaderMap, Response, StatusCode};
use tracing::error;

use crate::{AuthValidator, JwtSecret};

/// Implements JWT validation logics and integrates
/// to an Http [`AuthLayer`][crate::layers::AuthLayer]
/// by implementing the [`AuthValidator`] trait.
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct JwtAuthValidator {
    secret: JwtSecret,
}

impl JwtAuthValidator {
    /// Creates a new instance of [`JwtAuthValidator`].
    /// Validation logics are implemnted inside the `secret`
    /// argument (see [`JwtSecret`]).
    pub fn new(secret: JwtSecret) -> Self {
        Self { secret }
    }
}

impl AuthValidator for JwtAuthValidator {
    type ResponseBody = hyper::Body;

    fn validate(&self, headers: &HeaderMap) -> Result<(), Response<Self::ResponseBody>> {
        match get_bearer(headers) {
            Some(jwt) => match self.secret.validate(jwt) {
                Ok(_) => Ok(()),
                Err(e) => {
                    error!(target = "engine::jwt-validator", "{e}");
                    let response = err_response();
                    Err(response)
                }
            },
            None => {
                error!(target = "engine::jwt-validator", "Missing JWT header");
                let response = err_response();
                Err(response)
            }
        }
    }
}

/// This is an utility function that retrieves a bearer
/// token from an authorization Http header.
fn get_bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .map(|header| header.to_str())
        .and_then(|result| result.ok())
        .and_then(|auth| {
            let prefix = "Bearer ";
            auth.find(prefix)
                .map(|index| &auth[index + prefix.len()..])
                .map(|token| token.to_owned())
        })
}

fn err_response() -> Response<hyper::Body> {
    let body = hyper::Body::empty();
    // We build a static response.
    // So we are safe to "expect" on the result
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .body(body)
        .expect("This should never happen")
}

#[cfg(test)]
mod tests {
    use http::{header, HeaderMap};

    use crate::layers::jwt_validator::get_bearer;

    #[test]
    fn auth_header_available() {
        let jwt = "foo";
        let bearer = format!("Bearer {jwt}");
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, bearer.parse().unwrap());
        let token = get_bearer(&headers).unwrap();
        assert_eq!(token, jwt);
    }

    #[test]
    fn auth_header_not_available() {
        let headers = HeaderMap::new();
        let token = get_bearer(&headers);
        assert!(matches!(token, None));
    }

    #[test]
    fn auth_header_malformed() {
        let jwt = "foo";
        let bearer = format!("Bea___rer {jwt}");
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, bearer.parse().unwrap());
        let token = get_bearer(&headers);
        assert!(matches!(token, None));
    }
}
