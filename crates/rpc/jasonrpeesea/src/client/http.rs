use super::{
    decode, decode_batch, write_batch, write_request, BatchRequestBuilder, ClientT, Error,
    RawResponse, Subscription, SubscriptionClientT, ToRpcParams,
};
use crate::ErrorObject;
use reqwest::{
    header::{HeaderMap, HeaderValue, CONTENT_TYPE},
    IntoUrl, Url,
};
use serde::de::DeserializeOwned;
use std::{
    fmt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

type HeaderFn = dyn Fn() -> HeaderMap + Send + Sync;

/// Builds an [`HttpClient`].
#[derive(Clone)]
pub struct HttpClientBuilder {
    headers: HeaderMap,
    header_fn: Option<Arc<HeaderFn>>,
    request_timeout: Duration,
}

impl Default for HttpClientBuilder {
    fn default() -> Self {
        Self {
            headers: HeaderMap::new(),
            header_fn: None,
            request_timeout: Duration::from_secs(60),
        }
    }
}

impl fmt::Debug for HttpClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpClientBuilder")
            .field("headers", &self.headers)
            .field("request_timeout", &self.request_timeout)
            .finish_non_exhaustive()
    }
}

impl HttpClientBuilder {
    /// Sets headers sent with every request.
    pub fn set_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    /// Sets a function that returns additional headers for every request, such as a fresh
    /// authentication token.
    pub fn set_headers_fn(mut self, f: impl Fn() -> HeaderMap + Send + Sync + 'static) -> Self {
        self.header_fn = Some(Arc::new(f));
        self
    }

    /// Sets the request timeout. Default is 60 seconds.
    pub const fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Creates a client for the given URL.
    pub fn build(self, url: impl IntoUrl) -> Result<HttpClient, Error> {
        let mut headers = self.headers;
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(self.request_timeout)
            .build()
            .map_err(|err| Error::Transport(err.into()))?;
        let url = url.into_url().map_err(|err| Error::Transport(err.into()))?;
        Ok(HttpClient { client, url, header_fn: self.header_fn, next_id: Arc::default() })
    }
}

/// A JSON-RPC client over HTTP.
#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
    url: Url,
    header_fn: Option<Arc<HeaderFn>>,
    next_id: Arc<AtomicU64>,
}

impl fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpClient").field("url", &self.url).finish_non_exhaustive()
    }
}

impl HttpClient {
    /// Returns a builder.
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::default()
    }

    fn next_ids(&self, n: u64) -> u64 {
        self.next_id.fetch_add(n, Ordering::Relaxed)
    }

    async fn post(&self, body: String) -> Result<bytes::Bytes, Error> {
        let mut request = self.client.post(self.url.clone()).body(body);
        if let Some(f) = &self.header_fn {
            request = request.headers(f());
        }
        let response = request.send().await.map_err(|err| {
            if err.is_timeout() {
                Error::RequestTimeout
            } else {
                Error::Transport(err.into())
            }
        })?;
        let status = response.status();
        let body = response.bytes().await.map_err(|err| Error::Transport(err.into()))?;
        // Error responses may still carry a JSON-RPC error.
        if !status.is_success() && serde_json::from_slice::<RawResponse<'_>>(&body).is_err() {
            return Err(Error::Transport(format!("request failed with status {status}").into()))
        }
        Ok(body)
    }
}

impl ClientT for HttpClient {
    async fn request<R, P>(&self, method: &str, params: P) -> Result<R, Error>
    where
        R: DeserializeOwned,
        P: ToRpcParams + Send,
    {
        let params = params.to_rpc_params()?;
        let mut body = String::new();
        write_request(&mut body, self.next_ids(1), method, params.as_deref());
        let body = self.post(body).await?;
        decode(serde_json::from_slice::<RawResponse<'_>>(&body)?.into_result())
    }

    async fn batch_request<R>(
        &self,
        batch: BatchRequestBuilder,
    ) -> Result<Vec<Result<R, ErrorObject>>, Error>
    where
        R: DeserializeOwned,
    {
        let first_id = self.next_ids(batch.0.len() as u64);
        let body = self.post(write_batch(&batch, first_id)).await?;
        let responses = match serde_json::from_slice::<Vec<RawResponse<'_>>>(&body) {
            Ok(responses) => responses,
            // The whole batch failed.
            Err(_) => {
                let err = serde_json::from_slice::<RawResponse<'_>>(&body)?.into_result();
                return Err(Error::Call(
                    err.err().unwrap_or_else(|| crate::ErrorCode::InvalidRequest.into()),
                ));
            }
        };
        let mut results = vec![Err(crate::ErrorCode::InvalidRequest.into()); batch.0.len()];
        for response in responses {
            if let Some(slot) = response
                .id
                .and_then(|id| id.checked_sub(first_id))
                .and_then(|i| results.get_mut(i as usize))
            {
                *slot = response.into_result();
            }
        }
        decode_batch(results)
    }
}

impl SubscriptionClientT for HttpClient {
    async fn subscribe<N, P>(&self, _: &str, _: P, _: &str) -> Result<Subscription<N>, Error>
    where
        N: DeserializeOwned,
        P: ToRpcParams + Send,
    {
        Err(Error::Unsupported("subscriptions"))
    }
}
