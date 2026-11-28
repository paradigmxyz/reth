use jsonrpsee::{
    core::middleware::{Batch, Notification},
    server::middleware::rpc::RpcServiceT,
    types::{
        ErrorObjectOwned, Id, Request, Response, ResponsePayload as TypesResponsePayload,
        INVALID_PARAMS_CODE,
    },
    BatchResponseBuilder, MethodResponse, ResponsePayload as ServerResponsePayload,
};
use serde_json::Value;
use std::future::Future;
use tower::Layer;

const INVALID_PARAMS_MESSAGE: &str = "Invalid params";

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NormalizeInvalidParamsLayer;

impl<S> Layer<S> for NormalizeInvalidParamsLayer {
    type Service = NormalizeInvalidParamsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        NormalizeInvalidParamsService { inner }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NormalizeInvalidParamsService<S> {
    inner: S,
}

impl<S> RpcServiceT for NormalizeInvalidParamsService<S>
where
    S: RpcServiceT<
            MethodResponse = MethodResponse,
            BatchResponse = MethodResponse,
            NotificationResponse = MethodResponse,
        > + Send
        + Sync
        + Clone
        + 'static,
{
    type MethodResponse = MethodResponse;
    type BatchResponse = MethodResponse;
    type NotificationResponse = MethodResponse;

    fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = MethodResponse> + Send + 'a {
        let fut = self.inner.call(req);
        async move { normalize_response(fut.await) }
    }

    fn batch<'a>(&self, batch: Batch<'a>) -> impl Future<Output = MethodResponse> + Send + 'a {
        let fut = self.inner.batch(batch);
        async move { normalize_response(fut.await) }
    }

    fn notification<'a>(
        &self,
        notification: Notification<'a>,
    ) -> impl Future<Output = MethodResponse> + Send + 'a {
        self.inner.notification(notification)
    }
}

fn normalize_response(response: MethodResponse) -> MethodResponse {
    if response.is_batch() {
        return normalize_batch_response(response)
    }

    if response.as_error_code() != Some(INVALID_PARAMS_CODE) {
        return response
    }

    let Some((id, message)) = normalized_error(response.as_json().get()) else { return response };

    MethodResponse::error(
        id,
        ErrorObjectOwned::owned(INVALID_PARAMS_CODE, message, Option::<()>::None),
    )
}

fn normalize_batch_response(response: MethodResponse) -> MethodResponse {
    let Ok(parsed) = serde_json::from_str::<Vec<Response<'_, Value>>>(response.as_json().get())
    else {
        return response
    };

    if !parsed.iter().any(|item| {
        matches!(
            &item.payload,
            TypesResponsePayload::Error(error)
                if normalized_message(error).is_some()
        )
    }) {
        return response
    }

    let mut builder = BatchResponseBuilder::new_with_limit(usize::MAX);
    for item in parsed {
        let id = item.id.into_owned();
        let response = match item.payload {
            TypesResponsePayload::Success(value) => MethodResponse::response(
                id,
                ServerResponsePayload::success(value.into_owned()),
                usize::MAX,
            ),
            TypesResponsePayload::Error(error) => {
                if let Some(message) = normalized_message(&error) {
                    MethodResponse::error(
                        id,
                        ErrorObjectOwned::owned(INVALID_PARAMS_CODE, message, Option::<()>::None),
                    )
                } else {
                    MethodResponse::error(id, error.into_owned())
                }
            }
        };
        builder.append(response).expect("usize::MAX cannot reject a rebuilt batch response");
    }

    MethodResponse::from_batch(builder.finish())
}

fn normalized_error(raw: &str) -> Option<(Id<'static>, String)> {
    let response = serde_json::from_str::<Response<'_, Value>>(raw).ok()?;
    let TypesResponsePayload::Error(error) = response.payload else { return None };
    let message = normalized_message(&error)?;
    Some((response.id.into_owned(), message))
}

fn normalized_message(error: &jsonrpsee::types::ErrorObject<'_>) -> Option<String> {
    if error.code() != INVALID_PARAMS_CODE || error.message() != INVALID_PARAMS_MESSAGE {
        return None
    }
    let data = error.data()?;
    serde_json::from_str::<String>(data.get()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpsee::types::ErrorObject;

    #[test]
    fn rewrites_invalid_params_string_data() {
        let response = MethodResponse::error(
            Id::Number(1),
            ErrorObject::owned(
                INVALID_PARAMS_CODE,
                INVALID_PARAMS_MESSAGE,
                Some("hex string without 0x prefix"),
            ),
        );

        let normalized = normalize_response(response);
        let value: Value = serde_json::from_str(normalized.as_json().get()).unwrap();

        assert_eq!(value["error"]["code"], INVALID_PARAMS_CODE);
        assert_eq!(value["error"]["message"], "hex string without 0x prefix");
        assert!(value["error"].get("data").is_none());
    }

    #[test]
    fn leaves_non_matching_errors_unchanged() {
        let response = MethodResponse::error(
            Id::Number(1),
            ErrorObject::owned(INVALID_PARAMS_CODE, "custom invalid params", Some("details")),
        );
        let before = response.as_json().get().to_owned();

        let normalized = normalize_response(response);

        assert_eq!(normalized.as_json().get(), before);
    }

    #[test]
    fn rewrites_matching_errors_inside_batch() {
        let mut batch = BatchResponseBuilder::new_with_limit(usize::MAX);
        let bad = MethodResponse::error(
            Id::Number(1),
            ErrorObject::owned(INVALID_PARAMS_CODE, INVALID_PARAMS_MESSAGE, Some("bad key")),
        );
        let ok = MethodResponse::response(
            Id::Number(2),
            ServerResponsePayload::success(serde_json::json!({"ok": true})),
            usize::MAX,
        );
        batch.append(bad).unwrap();
        batch.append(ok).unwrap();

        let normalized = normalize_response(MethodResponse::from_batch(batch.finish()));
        let value: Value = serde_json::from_str(normalized.as_json().get()).unwrap();

        assert_eq!(value[0]["error"]["message"], "bad key");
        assert!(value[0]["error"].get("data").is_none());
        assert_eq!(value[1]["result"]["ok"], true);
    }
}
