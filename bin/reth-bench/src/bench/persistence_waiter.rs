//! Persistence waiting utilities for benchmarks.
//!
//! Provides waiting behavior to control benchmark pacing:
//! - **Fixed duration waits**: Sleep for a fixed time between blocks
//! - **Persistence-based waits**: Wait for blocks to be persisted using
//!   `reth_subscribePersistedBlock` subscription
//! - **Combined mode**: Wait at least the fixed duration, and also wait for persistence if the
//!   block hasn't been persisted yet (whichever takes longer)

use alloy_eips::BlockNumHash;
use alloy_network::Ethereum;
use alloy_provider::{Provider, RootProvider};
use alloy_pubsub::SubscriptionStream;
use alloy_rpc_client::RpcClient;
use alloy_transport_ws::WsConnect;
use eyre::Context;
use futures::StreamExt;
use std::time::Duration;
use tracing::{debug, info};

/// Default `WebSocket` RPC port for reth.
const DEFAULT_WS_RPC_PORT: u16 = 8546;
use url::Url;

/// Returns the websocket RPC URL used for the persistence subscription.
///
/// Preference:
/// - If `ws_rpc_url` is provided, use it directly.
/// - Otherwise, derive a WS RPC URL from `engine_rpc_url`.
///
/// The persistence subscription endpoint (`reth_subscribePersistedBlock`) is exposed on
/// the regular RPC server (WS port, usually 8546), not on the engine API port (usually 8551).
/// Since we may only have the engine URL by default, we convert the scheme
/// (http→ws, https→wss) and force the port to 8546.
pub(crate) fn derive_ws_rpc_url(
    ws_rpc_url: Option<&str>,
    engine_rpc_url: &str,
) -> eyre::Result<Url> {
    if let Some(ws_url) = ws_rpc_url {
        let parsed: Url = ws_url
            .parse()
            .wrap_err_with(|| format!("Failed to parse WebSocket RPC URL: {ws_url}"))?;
        info!(target: "reth-bench", ws_url = %parsed, "Using provided WebSocket RPC URL");
        Ok(parsed)
    } else {
        let derived = engine_url_to_ws_url(engine_rpc_url)?;
        debug!(
            target: "reth-bench",
            engine_url = %engine_rpc_url,
            %derived,
            "Derived WebSocket RPC URL from engine RPC URL"
        );
        Ok(derived)
    }
}

/// Converts an engine API URL to the default RPC websocket URL.
///
/// Transformations:
/// - `http`  → `ws`
/// - `https` → `wss`
/// - `ws` / `wss` keep their scheme
/// - Port is always set to `8546`, reth's default RPC websocket port.
///
/// This is used when we only know the engine API URL (typically `:8551`) but
/// need to connect to the node's WS RPC endpoint for persistence events.
fn engine_url_to_ws_url(engine_url: &str) -> eyre::Result<Url> {
    let url: Url = engine_url
        .parse()
        .wrap_err_with(|| format!("Failed to parse engine RPC URL: {engine_url}"))?;

    let mut ws_url = url.clone();

    match ws_url.scheme() {
        "http" => ws_url
            .set_scheme("ws")
            .map_err(|_| eyre::eyre!("Failed to set WS scheme for URL: {url}"))?,
        "https" => ws_url
            .set_scheme("wss")
            .map_err(|_| eyre::eyre!("Failed to set WSS scheme for URL: {url}"))?,
        "ws" | "wss" => {}
        scheme => {
            return Err(eyre::eyre!(
            "Unsupported URL scheme '{scheme}' for URL: {url}. Expected http, https, ws, or wss."
        ))
        }
    }

    ws_url
        .set_port(Some(DEFAULT_WS_RPC_PORT))
        .map_err(|_| eyre::eyre!("Failed to set port for URL: {url}"))?;

    Ok(ws_url)
}

/// Waits until the persistence subscription reports that `target` has been persisted.
///
/// Consumes subscription events until `last_persisted >= target`, or returns an error if:
/// - the subscription stream ends unexpectedly, or
/// - `timeout` elapses before `target` is observed.
async fn wait_for_persistence(
    stream: &mut SubscriptionStream<BlockNumHash>,
    target: u64,
    last_persisted: &mut u64,
    timeout: Duration,
) -> eyre::Result<()> {
    tokio::time::timeout(timeout, async {
        while *last_persisted < target {
            match stream.next().await {
                Some(persisted) => {
                    *last_persisted = persisted.number;
                    debug!(
                        target: "reth-bench",
                        persisted_block = ?last_persisted,
                        "Received persistence notification"
                    );
                }
                None => {
                    return Err(eyre::eyre!("Persistence subscription closed unexpectedly"));
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| {
        eyre::eyre!(
            "Persistence timeout: target block {} not persisted within {:?}. Last persisted: {}",
            target,
            timeout,
            last_persisted
        )
    })?
}

/// Wrapper that keeps both the subscription stream and the underlying provider alive.
/// The provider must be kept alive for the subscription to continue receiving events.
pub(crate) struct PersistenceSubscription {
    _provider: RootProvider<Ethereum>,
    stream: SubscriptionStream<BlockNumHash>,
}

impl PersistenceSubscription {
    const fn new(
        provider: RootProvider<Ethereum>,
        stream: SubscriptionStream<BlockNumHash>,
    ) -> Self {
        Self { _provider: provider, stream }
    }

    const fn stream_mut(&mut self) -> &mut SubscriptionStream<BlockNumHash> {
        &mut self.stream
    }
}

/// Establishes a websocket connection and subscribes to `reth_subscribePersistedBlock`.
///
/// The `keepalive_interval` is set to match `persistence_timeout` so that the `WebSocket`
/// connection is not dropped during long MDBX commits that block the server from responding
/// to pings.
pub(crate) async fn setup_persistence_subscription(
    ws_url: Url,
    _persistence_timeout: Duration,
) -> eyre::Result<PersistenceSubscription> {
    info!(target: "reth-bench", "Connecting to WebSocket at {} for persistence subscription", ws_url);

    let ws_connect = WsConnect::new(ws_url.to_string());
    let client = RpcClient::connect_pubsub(ws_connect)
        .await
        .wrap_err("Failed to connect to WebSocket RPC endpoint")?;
    let provider: RootProvider<Ethereum> = RootProvider::new(client);

    let subscription = provider
        .subscribe_to::<BlockNumHash>("reth_subscribePersistedBlock")
        .await
        .wrap_err("Failed to subscribe to persistence notifications")?;

    info!(target: "reth-bench", "Subscribed to persistence notifications");

    Ok(PersistenceSubscription::new(provider, subscription.into_stream()))
}

/// Encapsulates the block waiting logic.
///
/// Provides a simple `on_block()` interface that handles both:
/// - Fixed duration waits (when `wait_time` is set)
/// - Persistence-based waits (when `subscription` is set)
///
/// For persistence mode, waits after every `(threshold + 1)` blocks.
pub(crate) struct PersistenceWaiter {
    wait_time: Option<Duration>,
    subscription: Option<PersistenceSubscription>,
    blocks_sent: u64,
    last_persisted: u64,
    threshold: u64,
    timeout: Duration,
}

impl PersistenceWaiter {
    pub(crate) const fn with_duration(wait_time: Duration) -> Self {
        Self {
            wait_time: Some(wait_time),
            subscription: None,
            blocks_sent: 0,
            last_persisted: 0,
            threshold: 0,
            timeout: Duration::ZERO,
        }
    }

    pub(crate) const fn with_subscription(
        subscription: PersistenceSubscription,
        threshold: u64,
        timeout: Duration,
    ) -> Self {
        Self {
            wait_time: None,
            subscription: Some(subscription),
            blocks_sent: 0,
            last_persisted: 0,
            threshold,
            timeout,
        }
    }

    /// Creates a waiter that combines both duration and persistence waiting.
    ///
    /// Waits at least `wait_time` between blocks, and also waits for persistence
    /// if the block hasn't been persisted yet (whichever takes longer).
    pub(crate) const fn with_duration_and_subscription(
        wait_time: Duration,
        subscription: PersistenceSubscription,
        threshold: u64,
        timeout: Duration,
    ) -> Self {
        Self {
            wait_time: Some(wait_time),
            subscription: Some(subscription),
            blocks_sent: 0,
            last_persisted: 0,
            threshold,
            timeout,
        }
    }

    /// Called once per block. Waits based on the configured mode.
    ///
    /// When both `wait_time` and `subscription` are set (combined mode):
    /// - Always waits at least `wait_time`
    /// - Additionally waits for persistence if we're at a persistence checkpoint
    #[allow(clippy::manual_is_multiple_of)]
    pub(crate) async fn on_block(&mut self, block_number: u64) -> eyre::Result<()> {
        // Always wait for the fixed duration if configured
        if let Some(wait_time) = self.wait_time {
            tokio::time::sleep(wait_time).await;
        }

        // Check persistence if subscription is configured
        let Some(ref mut subscription) = self.subscription else {
            return Ok(());
        };

        self.blocks_sent += 1;

        if self.blocks_sent % (self.threshold + 1) == 0 {
            debug!(
                target: "reth-bench",
                target_block = ?block_number,
                last_persisted = self.last_persisted,
                blocks_sent = self.blocks_sent,
                "Waiting for persistence"
            );

            wait_for_persistence(
                subscription.stream_mut(),
                block_number,
                &mut self.last_persisted,
                self.timeout,
            )
            .await?;

            debug!(
                target: "reth-bench",
                persisted = self.last_persisted,
                "Persistence caught up"
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_engine_url_to_ws_url() {
        // http -> ws, always uses port 8546
        let result = engine_url_to_ws_url("http://localhost:8551").unwrap();
        assert_eq!(result.as_str(), "ws://localhost:8546/");

        // https -> wss
        let result = engine_url_to_ws_url("https://localhost:8551").unwrap();
        assert_eq!(result.as_str(), "wss://localhost:8546/");

        // Custom engine port still maps to 8546
        let result = engine_url_to_ws_url("http://localhost:9551").unwrap();
        assert_eq!(result.port(), Some(8546));

        // Already ws passthrough
        let result = engine_url_to_ws_url("ws://localhost:8546").unwrap();
        assert_eq!(result.scheme(), "ws");

        // Invalid inputs
        assert!(engine_url_to_ws_url("ftp://localhost:8551").is_err());
        assert!(engine_url_to_ws_url("not a valid url").is_err());
    }

    #[tokio::test]
    async fn test_waiter_with_duration() {
        let mut waiter = PersistenceWaiter::with_duration(Duration::from_millis(1));

        let start = Instant::now();
        waiter.on_block(1).await.unwrap();
        waiter.on_block(2).await.unwrap();
        waiter.on_block(3).await.unwrap();

        // Should have waited ~3ms total
        assert!(start.elapsed() >= Duration::from_millis(3));
    }
}
