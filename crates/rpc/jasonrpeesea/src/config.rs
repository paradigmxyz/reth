use crate::{IdProvider, RandomIntegerIdProvider};
use std::sync::Arc;
use tokio::runtime::Handle;

/// Server configuration.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub(crate) max_request_body_size: u32,
    pub(crate) max_response_body_size: u32,
    pub(crate) max_connections: u32,
    pub(crate) max_subscriptions_per_connection: u32,
    pub(crate) message_buffer_capacity: u32,
    pub(crate) enable_http: bool,
    pub(crate) enable_ws: bool,
    pub(crate) id_provider: Arc<dyn IdProvider>,
    pub(crate) tokio_runtime: Option<Handle>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_request_body_size: 10 * 1024 * 1024,
            max_response_body_size: 10 * 1024 * 1024,
            max_connections: 100,
            max_subscriptions_per_connection: 1024,
            message_buffer_capacity: 1024,
            enable_http: true,
            enable_ws: true,
            id_provider: Arc::new(RandomIntegerIdProvider),
            tokio_runtime: None,
        }
    }
}

impl ServerConfig {
    /// Sets the maximum size of a request in bytes. Default is 10 MiB.
    pub const fn max_request_body_size(mut self, size: u32) -> Self {
        self.max_request_body_size = size;
        self
    }

    /// Sets the maximum size of a response in bytes. Default is 10 MiB.
    pub const fn max_response_body_size(mut self, size: u32) -> Self {
        self.max_response_body_size = size;
        self
    }

    /// Sets the maximum number of connections. Default is 100.
    pub const fn max_connections(mut self, max: u32) -> Self {
        self.max_connections = max;
        self
    }

    /// Sets the maximum number of subscriptions per connection. Default is 1024.
    pub const fn max_subscriptions_per_connection(mut self, max: u32) -> Self {
        self.max_subscriptions_per_connection = max;
        self
    }

    /// Sets the number of messages that can be queued for sending per connection, which also
    /// bounds the number of concurrent calls. Default is 1024.
    pub const fn set_message_buffer_capacity(mut self, capacity: u32) -> Self {
        self.message_buffer_capacity = capacity;
        self
    }

    /// Only accepts HTTP requests.
    pub const fn http_only(mut self) -> Self {
        self.enable_http = true;
        self.enable_ws = false;
        self
    }

    /// Only accepts `WebSocket` connections.
    pub const fn ws_only(mut self) -> Self {
        self.enable_http = false;
        self.enable_ws = true;
        self
    }

    /// Sets the subscription id provider. Default is [`RandomIntegerIdProvider`].
    pub fn set_id_provider(mut self, id_provider: impl IdProvider + 'static) -> Self {
        self.id_provider = Arc::new(id_provider);
        self
    }

    /// Runs connections on the given runtime instead of the current one.
    pub fn custom_tokio_runtime(mut self, runtime: Handle) -> Self {
        self.tokio_runtime = Some(runtime);
        self
    }

    /// Returns the maximum size of a request in bytes.
    pub const fn max_request_size(&self) -> u32 {
        self.max_request_body_size
    }

    /// Returns the maximum size of a response in bytes.
    pub const fn max_response_size(&self) -> u32 {
        self.max_response_body_size
    }

    /// Returns the maximum number of connections.
    pub const fn connection_limit(&self) -> u32 {
        self.max_connections
    }

    /// Spawns `fut` on the configured runtime, or the current one.
    pub fn spawn<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        match &self.tokio_runtime {
            Some(runtime) => drop(runtime.spawn(fut)),
            None => drop(tokio::spawn(fut)),
        }
    }
}
