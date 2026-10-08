use super::{
    decode, decode_batch, write_batch, write_notification, write_request, BatchRequestBuilder,
    ClientT, Error, RawResponse, Subscription, SubscriptionClientT, ToRpcParams,
};
use crate::ErrorObject;
use reqwest::{
    header::{HeaderMap, HeaderValue, CONTENT_TYPE},
    IntoUrl, Response, Url,
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
use tokio::sync::Semaphore;

type HeaderFn = dyn Fn() -> HeaderMap + Send + Sync;

const TEN_MIB: usize = 10 * 1024 * 1024;

/// Builds an [`HttpClient`].
#[derive(Clone)]
pub struct HttpClientBuilder {
    headers: HeaderMap,
    header_fn: Option<Arc<HeaderFn>>,
    request_timeout: Duration,
    max_request_size: usize,
    max_response_size: usize,
    max_concurrent_requests: Option<usize>,
}

impl Default for HttpClientBuilder {
    fn default() -> Self {
        Self {
            headers: HeaderMap::new(),
            header_fn: None,
            request_timeout: Duration::from_secs(60),
            max_request_size: TEN_MIB,
            max_response_size: TEN_MIB,
            max_concurrent_requests: None,
        }
    }
}

impl fmt::Debug for HttpClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpClientBuilder")
            .field("headers", &self.headers)
            .field("request_timeout", &self.request_timeout)
            .field("max_request_size", &self.max_request_size)
            .field("max_response_size", &self.max_response_size)
            .field("max_concurrent_requests", &self.max_concurrent_requests)
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

    /// Sets the maximum size of a request body in bytes. Default is 10 MiB.
    pub const fn max_request_size(mut self, size: usize) -> Self {
        self.max_request_size = size;
        self
    }

    /// Sets the maximum size of a response body in bytes. Default is 10 MiB.
    pub const fn max_response_size(mut self, size: usize) -> Self {
        self.max_response_size = size;
        self
    }

    /// Sets the maximum number of requests awaiting a response. Default is unlimited.
    ///
    /// Further requests wait until an earlier one completes.
    pub const fn max_concurrent_requests(mut self, max: usize) -> Self {
        self.max_concurrent_requests = Some(max);
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
        Ok(HttpClient {
            client,
            url,
            header_fn: self.header_fn,
            next_id: Arc::default(),
            max_request_size: self.max_request_size,
            max_response_size: self.max_response_size,
            requests: self
                .max_concurrent_requests
                .map(|max| Arc::new(Semaphore::new(max.min(Semaphore::MAX_PERMITS)))),
        })
    }
}

/// A JSON-RPC client over HTTP.
#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
    url: Url,
    header_fn: Option<Arc<HeaderFn>>,
    next_id: Arc<AtomicU64>,
    max_request_size: usize,
    max_response_size: usize,
    requests: Option<Arc<Semaphore>>,
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

    async fn post(&self, body: String) -> Result<Vec<u8>, Error> {
        if body.len() > self.max_request_size {
            return Err(Error::Transport(
                format!("request exceeds the limit of {} bytes", self.max_request_size).into(),
            ))
        }
        let _permit =
            if let Some(requests) = &self.requests { requests.acquire().await.ok() } else { None };
        let mut request = self.client.post(self.url.clone()).body(body);
        if let Some(f) = &self.header_fn {
            request = request.headers(f());
        }
        let response = request.send().await.map_err(transport_error)?;
        let status = response.status();
        let body = read_body(response, self.max_response_size).await?;
        // Error responses may still carry a JSON-RPC error.
        if !status.is_success() && serde_json::from_slice::<RawResponse<'_>>(&body).is_err() {
            return Err(Error::Transport(format!("request failed with status {status}").into()))
        }
        Ok(body)
    }
}

/// Reads the body of `response`, failing once it exceeds `max` bytes.
async fn read_body(mut response: Response, max: usize) -> Result<Vec<u8>, Error> {
    let too_large =
        || Error::Transport(format!("response exceeds the limit of {max} bytes").into());
    let len = response.content_length().unwrap_or(0);
    if len > max as u64 {
        return Err(too_large())
    }
    let mut body = Vec::with_capacity(len as usize);
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if chunk.len() > max - body.len() {
            return Err(too_large())
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn transport_error(err: reqwest::Error) -> Error {
    if err.is_timeout() {
        Error::RequestTimeout
    } else {
        Error::Transport(err.into())
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

    async fn notification<P>(&self, method: &str, params: P) -> Result<(), Error>
    where
        P: ToRpcParams + Send,
    {
        let params = params.to_rpc_params()?;
        let mut body = String::new();
        write_notification(&mut body, method, params.as_deref());
        self.post(body).await.map(drop)
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

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::{rpc_params, server::ServerBuilder, RpcModule, ServerHandle};

    async fn start(module: RpcModule) -> (String, ServerHandle) {
        let server = ServerBuilder::new().build("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", server.local_addr().unwrap());
        (url, server.start(module))
    }

    #[tokio::test]
    async fn notification_and_size_limits() {
        let mut module = RpcModule::new();
        module.register_method("echo", |params| params.one::<String>()).unwrap();
        let (url, _handle) = start(module).await;

        let client = HttpClientBuilder::default().max_request_size(200).build(&url).unwrap();
        client.notification("echo", rpc_params!["a"]).await.unwrap();
        let a = "a".repeat(50);
        assert_eq!(client.request::<String, _>("echo", rpc_params![&a]).await.unwrap(), a);
        let err = client.request::<String, _>("echo", rpc_params!["a".repeat(200)]).await;
        assert_eq!(
            err.unwrap_err().to_string(),
            "transport error: request exceeds the limit of 200 bytes"
        );

        let client = HttpClientBuilder::default().max_response_size(50).build(&url).unwrap();
        let err = client.request::<String, _>("echo", rpc_params![&a]).await.unwrap_err();
        assert_eq!(err.to_string(), "transport error: response exceeds the limit of 50 bytes");
    }

    #[tokio::test]
    async fn max_concurrent_requests() {
        let running = Arc::new(Semaphore::new(1));
        let mut module = RpcModule::new();
        module
            .register_async_method("alone", move |_| {
                let running = Arc::clone(&running);
                async move {
                    let permit = running.try_acquire();
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    permit.is_ok()
                }
            })
            .unwrap();
        let (url, _handle) = start(module).await;

        let client = HttpClientBuilder::default().max_concurrent_requests(1).build(url).unwrap();
        let requests = (0..4).map(|_| client.request::<bool, _>("alone", rpc_params![]));
        for result in futures::future::join_all(requests).await {
            assert!(result.unwrap());
        }
    }
}
