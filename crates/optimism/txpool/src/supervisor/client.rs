//! This is our custom implementation of validator struct

use crate::{
    interop::MaybeInteropTransaction,
    supervisor::{
        metrics::SupervisorMetrics, parse_access_list_items_to_inbox_entries, ExecutingDescriptor,
        InteropTxValidatorError,
    },
    InvalidCrossTx,
};
use alloy_consensus::Transaction;
use alloy_eips::eip2930::AccessList;
use alloy_primitives::{TxHash, B256};
use alloy_rpc_client::ReqwestClient;
use futures_util::{
    future::BoxFuture,
    stream::{self, StreamExt},
    Stream,
};
use op_alloy_consensus::interop::SafetyLevel;
use reth_transaction_pool::PoolTransaction;
use std::{
    borrow::Cow,
    future::IntoFuture,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::trace;

/// Supervisor hosted by op-labs
// TODO: This should be changes to actual supervisor url
pub const DEFAULT_SUPERVISOR_URL: &str = "http://localhost:1337/";

/// The default request timeout to use
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_millis(100);

/// Implementation of the supervisor trait for the interop.
#[derive(Debug, Clone)]
pub struct SupervisorClient {
    /// Stores type's data.
    inner: Arc<SupervisorClientInner>,
}

impl SupervisorClient {
    /// Returns a new [`SupervisorClientBuilder`].
    pub fn builder(supervisor_endpoint: impl Into<String>) -> SupervisorClientBuilder {
        SupervisorClientBuilder::new(supervisor_endpoint)
    }

    /// Returns configured timeout. See [`SupervisorClientInner`].
    pub fn timeout(&self) -> Duration {
        self.inner.timeout
    }

    /// Returns configured minimum safety level. See [`SupervisorClient`].
    pub fn safety(&self) -> SafetyLevel {
        self.inner.safety
    }

    /// Executes a `supervisor_checkAccessList` with the configured safety level.
    pub fn check_access_list<'a>(
        &self,
        inbox_entries: &'a [B256],
        executing_descriptor: ExecutingDescriptor,
    ) -> CheckAccessListRequest<'a> {
        CheckAccessListRequest {
            client: self.inner.client.clone(),
            inbox_entries: Cow::Borrowed(inbox_entries),
            executing_descriptor,
            timeout: self.inner.timeout,
            safety: self.inner.safety,
            metrics: self.inner.metrics.clone(),
        }
    }

    /// Extracts commitment from access list entries, pointing to 0x420..022 and validates them
    /// against supervisor.
    ///
    /// If commitment present pre-interop tx rejected.
    ///
    /// Returns:
    /// None - if tx is not cross chain,
    /// Some(Ok(()) - if tx is valid cross chain,
    /// Some(Err(e)) - if tx is not valid or interop is not active
    pub async fn is_valid_cross_tx(
        &self,
        access_list: Option<&AccessList>,
        hash: &TxHash,
        timestamp: u64,
        timeout: Option<u64>,
        is_interop_active: bool,
    ) -> Option<Result<(), InvalidCrossTx>> {
        // We don't need to check for deposit transaction in here, because they won't come from
        // txpool
        let access_list = access_list?;
        let inbox_entries = parse_access_list_items_to_inbox_entries(access_list.iter())
            .copied()
            .collect::<Vec<_>>();
        if inbox_entries.is_empty() {
            return None;
        }

        // Interop check
        if !is_interop_active {
            // No cross chain tx allowed before interop
            return Some(Err(InvalidCrossTx::CrossChainTxPreInterop))
        }

        if let Err(err) = self
            .check_access_list(
                inbox_entries.as_slice(),
                ExecutingDescriptor::new(timestamp, timeout),
            )
            .await
        {
            trace!(target: "txpool", hash=%hash, err=%err, "Cross chain transaction invalid");
            return Some(Err(InvalidCrossTx::ValidationError(err)));
        }
        Some(Ok(()))
    }

    /// Creates a stream that revalidates interop transactions against the supervisor.
    /// Returns
    /// An implementation of `Stream` that is `Send`-able and tied to the lifetime `'a` of `self`.
    /// Each item yielded by the stream is a tuple `(TItem, Option<Result<(), InvalidCrossTx>>)`.
    ///   - The first element is the original `TItem` that was revalidated.
    ///   - The second element is the `Option<Result<(), InvalidCrossTx>>` describes the outcome
    ///     - `None`: Transaction was not identified as a cross-chain candidate by initial checks.
    ///     - `Some(Ok(()))`: Supervisor confirmed the transaction is valid.
    ///     - `Some(Err(InvalidCrossTx))`: Supervisor indicated the transaction is invalid.
    pub fn revalidate_interop_txs_stream<'a, TItem, InputIter>(
        &'a self,
        txs_to_revalidate: InputIter,
        current_timestamp: u64,
        revalidation_window: u64,
        max_concurrent_queries: usize,
    ) -> impl Stream<Item = (TItem, Option<Result<(), InvalidCrossTx>>)> + Send + 'a
    where
        InputIter: IntoIterator<Item = TItem> + Send + 'a,
        InputIter::IntoIter: Send + 'a,
        TItem:
            MaybeInteropTransaction + PoolTransaction + Transaction + Clone + Send + Sync + 'static,
    {
        stream::iter(txs_to_revalidate.into_iter().map(move |tx_item| {
            let client_for_async_task = self.clone();

            async move {
                let validation_result = client_for_async_task
                    .is_valid_cross_tx(
                        tx_item.access_list(),
                        tx_item.hash(),
                        current_timestamp,
                        Some(revalidation_window),
                        true,
                    )
                    .await;

                // return the original transaction paired with its validation result.
                (tx_item, validation_result)
            }
        }))
        .buffered(max_concurrent_queries)
    }
}

/// Holds supervisor data. Inner type of [`SupervisorClient`].
#[derive(Debug, Clone)]
pub struct SupervisorClientInner {
    client: ReqwestClient,
    /// The default
    safety: SafetyLevel,
    /// The default request timeout
    timeout: Duration,
    /// Metrics for tracking supervisor operations
    metrics: SupervisorMetrics,
}

/// Builds [`SupervisorClient`].
#[derive(Debug)]
pub struct SupervisorClientBuilder {
    /// Supervisor server's socket.
    endpoint: String,
    /// Timeout for requests.
    ///
    /// NOTE: this timeout is only effective if it's shorter than the timeout configured for the
    /// underlying [`ReqwestClient`].
    timeout: Duration,
    /// Minimum [`SafetyLevel`] of cross-chain transactions accepted by this client.
    safety: SafetyLevel,
}

impl SupervisorClientBuilder {
    /// Creates a new builder.
    pub fn new(supervisor_endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: supervisor_endpoint.into(),
            timeout: DEFAULT_REQUEST_TIMEOUT,
            safety: SafetyLevel::CrossUnsafe,
        }
    }

    /// Configures a custom timeout
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets minimum safety level to accept for cross chain transactions.
    pub const fn minimum_safety(mut self, min_safety: SafetyLevel) -> Self {
        self.safety = min_safety;
        self
    }

    /// Creates a new supervisor validator.
    pub async fn build(self) -> SupervisorClient {
        let Self { endpoint, timeout, safety } = self;

        let client = ReqwestClient::builder()
            .connect(endpoint.as_str())
            .await
            .expect("building supervisor client");

        SupervisorClient {
            inner: Arc::new(SupervisorClientInner {
                client,
                safety,
                timeout,
                metrics: SupervisorMetrics::default(),
            }),
        }
    }
}

/// A Request future that issues a `supervisor_checkAccessList` request.
#[derive(Debug, Clone)]
pub struct CheckAccessListRequest<'a> {
    client: ReqwestClient,
    inbox_entries: Cow<'a, [B256]>,
    executing_descriptor: ExecutingDescriptor,
    timeout: Duration,
    safety: SafetyLevel,
    metrics: SupervisorMetrics,
}

impl<'a> CheckAccessListRequest<'a> {
    /// Configures the timeout to use for the request if any.
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Configures the [`SafetyLevel`] for this request
    pub const fn with_safety(mut self, safety: SafetyLevel) -> Self {
        self.safety = safety;
        self
    }
}

impl<'a> IntoFuture for CheckAccessListRequest<'a> {
    type Output = Result<(), InteropTxValidatorError>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        let Self { client, inbox_entries, executing_descriptor, timeout, safety, metrics } = self;
        Box::pin(async move {
            let start = Instant::now();

            let result = tokio::time::timeout(
                timeout,
                client.request(
                    "supervisor_checkAccessList",
                    (inbox_entries, safety, executing_descriptor),
                ),
            )
            .await;
            metrics.record_supervisor_query(start.elapsed());

            result
                .map_err(|_| InteropTxValidatorError::Timeout(timeout.as_secs()))?
                .map_err(InteropTxValidatorError::from_json_rpc)
        })
    }
}
