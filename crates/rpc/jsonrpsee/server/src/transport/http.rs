use crate::{server::handle_rpc_call, BatchRequestConfig, HttpRequest, HttpResponse, LOG_TARGET};
use http::Method;
use hyper::body::Bytes;
use jsonrpsee_core::{
    http_helpers::{read_body, HttpError},
    middleware::RpcServiceT,
    BoxError,
};

/// Returns true if the `content_type` header indicates a valid JSON message.
///
/// Only the media type is compared. The parameter list after it may be spaced freely and may quote
/// its values, so comparing the whole header would reject spellings that mean the same type, such
/// as `application/json; charset="utf-8"`. The media type is matched as a prefix rather than split
/// off and trimmed, so rejecting another type never walks the whitespace a client put after it.
fn is_json(content_type: Option<&hyper::header::HeaderValue>) -> bool {
    content_type.and_then(|val| val.to_str().ok()).is_some_and(|content| {
        ["application/json", "application/json-rpc"].into_iter().any(|media_type| {
            content.split_at_checked(media_type.len()).is_some_and(|(prefix, params)| {
                prefix.eq_ignore_ascii_case(media_type) &&
                    matches!(params.trim_ascii_start().as_bytes(), [] | [b';', ..])
            })
        })
    })
}

/// Make JSON-RPC HTTP call with a service [`RpcServiceT`]
///
/// Fails if the HTTP request was a malformed JSON-RPC request.
pub async fn call_with_service<S, B>(
    request: HttpRequest<B>,
    batch_config: BatchRequestConfig,
    max_request_size: u32,
    rpc_service: &S,
) -> HttpResponse
where
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Data: Send,
    B::Error: Into<BoxError>,
    S: RpcServiceT + Send,
{
    // Only the `POST` method is allowed.
    match *request.method() {
        Method::POST if is_json(request.headers().get(hyper::header::CONTENT_TYPE)) => {
            let (parts, body) = request.into_parts();

            let (body, is_single) = match read_body(&parts.headers, body, max_request_size).await {
                Ok(r) => r,
                Err(HttpError::TooLarge) => return response::too_large(max_request_size),
                Err(HttpError::Malformed) => return response::malformed(),
                Err(HttpError::Stream(e)) => {
                    tracing::warn!(target: LOG_TARGET, "Internal error reading request body: {}", e);
                    return response::internal_error();
                }
            };

            let rp = handle_rpc_call(&body, is_single, batch_config, rpc_service).await;

            // If the response is empty it means that it was a notification or empty batch.
            // For HTTP these are just ACK:ed with a empty body.
            response::from_method_response(rp)
        }
        // Error scenarios:
        Method::POST => response::unsupported_content_type(),
        _ => response::method_not_allowed(),
    }
}

/// HTTP response helpers.
pub mod response {
    use jsonrpsee_core::server::MethodResponse;
    use jsonrpsee_types::{
        error::{reject_too_big_request, ErrorCode},
        ErrorObjectOwned, Id, Response, ResponsePayload,
    };

    use crate::{HttpBody, HttpResponse};

    const JSON: &str = "application/json; charset=utf-8";
    const TEXT: &str = "text/plain";

    /// Create a response for json internal error.
    pub fn internal_error() -> HttpResponse {
        let err = ResponsePayload::<()>::error(ErrorObjectOwned::from(ErrorCode::InternalError));
        let rp = Response::new(err, Id::Null);
        let error = serde_json::to_string(&rp).expect("built from known-good data; qed");

        from_template(hyper::StatusCode::INTERNAL_SERVER_ERROR, error, JSON)
    }

    /// Create a text/plain response for disallowed method used.
    pub fn method_not_allowed() -> HttpResponse {
        from_template(
            hyper::StatusCode::METHOD_NOT_ALLOWED,
            "Used HTTP Method is not allowed. POST is required\n",
            TEXT,
        )
    }

    /// Create a json response for oversized requests (413)
    pub fn too_large(limit: u32) -> HttpResponse {
        let err = ResponsePayload::<()>::error(reject_too_big_request(limit));
        let rp = Response::new(err, Id::Null);
        let error = serde_json::to_string(&rp).expect("JSON serialization infallible; qed");

        from_template(hyper::StatusCode::PAYLOAD_TOO_LARGE, error, JSON)
    }

    /// Create a json response for empty or malformed requests (400)
    pub fn malformed() -> HttpResponse {
        let rp = Response::new(ResponsePayload::<()>::error(ErrorCode::ParseError), Id::Null);
        let error = serde_json::to_string(&rp).expect("JSON serialization infallible; qed");

        from_template(hyper::StatusCode::BAD_REQUEST, error, JSON)
    }

    /// Create a response body.
    pub(crate) fn from_template(
        status: hyper::StatusCode,
        body: impl Into<HttpBody>,
        content_type: &'static str,
    ) -> HttpResponse {
        HttpResponse::builder()
            .status(status)
            .header("content-type", hyper::header::HeaderValue::from_static(content_type))
            .body(body.into())
            // Parsing `StatusCode` and `HeaderValue` is infallible but
            // parsing body content is not.
            .expect("Unable to parse response body for type conversion")
    }

    /// Create a response from a method response.
    pub fn from_method_response(rp: MethodResponse) -> HttpResponse {
        let body = rp.into_json();
        from_template(hyper::StatusCode::OK, body, JSON)
    }

    /// Create a response for unsupported content type.
    pub fn unsupported_content_type() -> HttpResponse {
        from_template(
            hyper::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Supplied content type is not allowed. Content-Type: application/json is required\n",
            TEXT,
        )
    }

    /// Create a response for when the server is busy and can't accept more requests.
    pub fn too_many_requests() -> HttpResponse {
        from_template(
            hyper::StatusCode::TOO_MANY_REQUESTS,
            "Too many connections. Please try again later.",
            TEXT,
        )
    }

    /// Create a response for when the server denied the request.
    pub fn denied() -> HttpResponse {
        from_template(hyper::StatusCode::FORBIDDEN, HttpBody::default(), TEXT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::HeaderValue;
    use test_case::test_case;

    #[test_case("application/json" ; "json")]
    #[test_case("application/json-rpc" ; "json_rpc")]
    #[test_case("APPLICATION/JSON" ; "uppercase")]
    #[test_case("application/json; charset=utf-8" ; "charset")]
    #[test_case("application/json;charset=utf-8" ; "charset_no_space")]
    #[test_case("application/json;  charset=UTF-8" ; "charset_two_spaces_uppercase")]
    #[test_case(r#"application/json; charset="utf-8""# ; "quoted_charset")]
    #[test_case("application/json ; charset=utf-8" ; "space_before_semicolon")]
    #[test_case("application/json-rpc; charset=utf-8" ; "json_rpc_charset")]
    #[test_case("application/json;charset=utf-8;foo=bar" ; "extra_parameter")]
    fn json_content_types_are_accepted_whatever_their_parameters(accepted: &str) {
        assert!(is_json(Some(&HeaderValue::from_str(accepted).unwrap())), "{accepted}");
    }

    #[test_case("" ; "empty")]
    #[test_case("text/plain" ; "text_plain")]
    #[test_case("application/jsonx" ; "json_suffix")]
    #[test_case("application/json x" ; "json_space_suffix")]
    #[test_case("application/json-rpcx" ; "json_rpc_suffix")]
    #[test_case("application/octet-stream; charset=utf-8" ; "octet_stream")]
    fn non_json_content_types_are_rejected(rejected: &str) {
        assert!(!is_json(Some(&HeaderValue::from_str(rejected).unwrap())), "{rejected}");
    }

    #[test]
    fn missing_content_type_is_rejected() {
        assert!(!is_json(None));
    }
}
