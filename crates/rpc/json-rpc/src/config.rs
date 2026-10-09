use crate::{IdProvider, RandomIntegerIdProvider};
use std::{sync::Arc, time::Duration};
use tokio::runtime::Handle;

/// Server configuration.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub(crate) max_request_body_size: u32,
    pub(crate) max_response_body_size: u32,
    pub(crate) max_connections: u32,
    pub(crate) max_subscriptions_per_connection: u32,
    pub(crate) message_buffer_capacity: u32,
    pub(crate) batch_config: BatchRequestConfig,
    pub(crate) ping_config: Option<PingConfig>,
    pub(crate) tcp_no_delay: bool,
    pub(crate) keep_alive: bool,
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
            batch_config: BatchRequestConfig::Unlimited,
            ping_config: None,
            tcp_no_delay: true,
            keep_alive: true,
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

    /// Sets the batch request limit. Default is [`BatchRequestConfig::Unlimited`].
    pub const fn set_batch_request_config(mut self, config: BatchRequestConfig) -> Self {
        self.batch_config = config;
        self
    }

    /// Sends `WebSocket` pings and closes connections that stop answering. Disabled by default.
    pub const fn enable_ws_ping(mut self, config: PingConfig) -> Self {
        self.ping_config = Some(config);
        self
    }

    /// Disables `WebSocket` pings.
    pub const fn disable_ws_ping(mut self) -> Self {
        self.ping_config = None;
        self
    }

    /// Sets `TCP_NODELAY` on accepted sockets. Default is `true`.
    pub const fn set_tcp_no_delay(mut self, no_delay: bool) -> Self {
        self.tcp_no_delay = no_delay;
        self
    }

    /// Enables HTTP/1 keep-alive. Default is `true`.
    pub const fn set_keep_alive(mut self, keep_alive: bool) -> Self {
        self.keep_alive = keep_alive;
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

    /// Returns the batch request limit.
    pub const fn batch_request_config(&self) -> BatchRequestConfig {
        self.batch_config
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

/// Limit on the number of calls in a batch request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BatchRequestConfig {
    /// Batches are rejected.
    Disabled,
    /// Batches with more calls are rejected.
    Limit(u32),
    /// Batches of any length are accepted.
    #[default]
    Unlimited,
}

/// `WebSocket` ping configuration.
///
/// A ping is sent every `ping_interval`. The connection is closed after `max_failures`
/// consecutive intervals in which nothing was received for longer than `inactive_limit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PingConfig {
    pub(crate) ping_interval: Duration,
    pub(crate) inactive_limit: Duration,
    pub(crate) max_failures: usize,
}

impl Default for PingConfig {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(30),
            inactive_limit: Duration::from_secs(40),
            max_failures: 1,
        }
    }
}

impl PingConfig {
    /// Creates the default configuration: a ping every 30 seconds, closing the connection after
    /// 40 seconds without any message.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the ping interval.
    pub const fn ping_interval(mut self, interval: Duration) -> Self {
        self.ping_interval = interval;
        self
    }

    /// Sets how long the connection may be silent before an interval counts as failed.
    pub const fn inactive_limit(mut self, limit: Duration) -> Self {
        self.inactive_limit = limit;
        self
    }

    /// Sets the number of consecutive failed intervals before the connection is closed. A value
    /// of zero is treated as one.
    pub const fn max_failures(mut self, max: usize) -> Self {
        self.max_failures = max;
        self
    }
}
