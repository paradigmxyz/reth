//! This is our custom implementation of validator struct

use crate::{
    supervisor::{
        parse_access_list_items_to_inbox_entries, ExecutingDescriptor, InteropTxValidatorError,
    },
    InvalidCrossTx,
};
use alloy_eips::eip2930::AccessList;
use alloy_primitives::{TxHash, B256};
use alloy_rpc_client::ReqwestClient;
use futures_util::future::BoxFuture;
use op_alloy_consensus::interop::SafetyLevel;
use std::{borrow::Cow, future::IntoFuture, sync::Arc, time::Duration};
use tracing::trace;

use super::metrics::SupervisorMetrics;

/// Supervisor hosted by op-labs
// TODO: This should be changes to actual supervisor url
pub const DEFAULT_SUPERVISOR_URL: &str = "http://localhost:1337/";

/// The default request timeout to use
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_millis(100);

/// Implementation of the supervisor trait for the interop.
#[derive(Debug, Clone)]
pub struct SupervisorClient {
    /// Stores type's data.
    inner: Arc<SupervisorClientInner>,
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
    /// Minimum [`SafetyLevel`] accepted by this client.
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
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets minimum safety level to accept for cross chain transactions.
    pub fn minimum_safety(mut self, min_safety: SafetyLevel) -> Self {
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

impl SupervisorClient {
    /// Returns locally configured minimum [`SafetyLevel`] to accept for cross chain transactions.
    pub fn safety(&self) -> SafetyLevel {
        self.inner.safety
    }

    #[doc(hidden)]
    /// Updates metrics from internal intermediary counter, which is reset.
    pub fn update_metrics(&self) {
        self.inner.metrics.update();
    }

    #[doc(hidden)]
    /// Tracks given error for metrics.
    ///
    /// NOTE: Does _not_ write to metrics. Writing updates must explicitly be done calling
    /// [`update_metrics`](Self::update_metrics) after this call.
    pub fn weak_update_metrics(&self, err: &InvalidCrossTx) {
        self.inner.metrics.maybe_increment_safety_level_counter(err)
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
}

/// Stores data for [`SupervisorClient`].
#[derive(Debug)]
pub struct SupervisorClientInner {
    client: ReqwestClient,
    /// The default
    safety: SafetyLevel,
    /// The default request timeout
    timeout: Duration,
    #[doc(hidden)]
    pub(crate) metrics: SupervisorMetrics,
}

/// A Request future that issues a `supervisor_checkAccessList` request.
#[derive(Debug, Clone)]
pub struct CheckAccessListRequest<'a> {
    client: ReqwestClient,
    inbox_entries: Cow<'a, [B256]>,
    executing_descriptor: ExecutingDescriptor,
    timeout: Duration,
    safety: SafetyLevel,
}

impl CheckAccessListRequest<'_> {
    /// Configures the timeout to use for the request if any.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Configures the [`SafetyLevel`] for this request
    pub fn with_safety(mut self, safety: SafetyLevel) -> Self {
        self.safety = safety;
        self
    }
}

impl<'a> IntoFuture for CheckAccessListRequest<'a> {
    type Output = Result<(), InteropTxValidatorError>;
    type IntoFuture = BoxFuture<'a, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        let Self { client, inbox_entries, executing_descriptor, timeout, safety } = self;
        Box::pin(async move {
            tokio::time::timeout(
                timeout,
                client.request(
                    "supervisor_checkAccessList",
                    (inbox_entries, safety, executing_descriptor),
                ),
            )
            .await
            .map_err(|_| InteropTxValidatorError::ValidationTimeout(timeout.as_secs()))?
            .map_err(InteropTxValidatorError::client)
        })
    }
}
