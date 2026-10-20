use crate::{
    components::verify_component_response,
    selection::{check_request, encoded_size, select_with_cancel},
    verify_receipt_selection, ComponentObject, SelectionError, SelectionLimits, SelectionOperation,
    SelectionRequest, SelectionResponse, TransactionReceiptRequest, TransactionReceiptResponse,
};
use alloy_primitives::B256;
use reth_chainspec::EthereumHardforks;
use reth_ethereum_primitives::EthPrimitives;
use reth_provider::providers::{BlockchainProvider, ProviderNodeTypes};
use reth_pureth_receipt::{
    CanonicalityStatus, HistoricalAcquisitionError, LookupError, ObjectKind, ProviderSnapshot,
    RethRootProvider, RethRootProviderError, RootContext,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(target_arch = "wasm32"))]
use crate::{select_component_with_cancel, ComponentSnapshot};
#[cfg(not(target_arch = "wasm32"))]
use alloy_consensus::Header;
#[cfg(not(target_arch = "wasm32"))]
use reth_chainspec::EthChainSpec;
#[cfg(not(target_arch = "wasm32"))]
use reth_provider::BalStore;
#[cfg(not(target_arch = "wasm32"))]
use reth_pureth_block::{
    ExecutionPayloadSnapshot, ExecutionSourceProvider, PayloadLimits, PayloadSourceError,
    PayloadSourceMetadata, SourcePayloadSnapshot,
};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    pub block_hash: B256,
    pub object: String,
    pub selection: SelectionRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryResponse {
    pub block_hash: B256,
    pub object: String,
    pub root_context: String,
    pub block_status: String,
    pub selection: SelectionResponse,
}

#[derive(Debug)]
pub enum QueryError {
    UnsupportedObject,
    Provider(LookupError),
    Acquisition(HistoricalAcquisitionError),
    #[cfg(not(target_arch = "wasm32"))]
    PayloadAcquisition(PayloadSourceError),
    Selection(SelectionError),
    InvalidSnapshot,
}

impl From<SelectionError> for QueryError {
    fn from(error: SelectionError) -> Self {
        Self::Selection(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResponseVerificationError {
    InvalidContext,
    Selection(SelectionError),
}

#[derive(Debug)]
pub struct QueryService<P> {
    provider: P,
    limits: SelectionLimits,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct RethPayloadProvider<N: ProviderNodeTypes<Primitives = EthPrimitives>, B> {
    provider: BlockchainProvider<N>,
    bal_store: B,
    limits: PayloadLimits,
}

pub trait QueryHandler {
    fn transaction_receipt_with_cancel(
        &self,
        _request: TransactionReceiptRequest,
        _cancelled: &AtomicBool,
    ) -> Result<TransactionReceiptResponse, QueryError> {
        Err(QueryError::UnsupportedObject)
    }

    fn transaction_receipt(
        &self,
        request: TransactionReceiptRequest,
    ) -> Result<TransactionReceiptResponse, QueryError> {
        self.transaction_receipt_with_cancel(request, &AtomicBool::new(false))
    }

    fn query_with_cancel(
        &self,
        request: QueryRequest,
        cancelled: &AtomicBool,
    ) -> Result<QueryResponse, QueryError>;

    fn query(&self, request: QueryRequest) -> Result<QueryResponse, QueryError> {
        self.query_with_cancel(request, &AtomicBool::new(false))
    }
}

impl<P> QueryService<P> {
    pub const fn with_limits(mut self, limits: SelectionLimits) -> Self {
        self.limits = limits;
        self
    }
}

impl<N> QueryService<RethRootProvider<N>>
where
    N: ProviderNodeTypes<Primitives = EthPrimitives>,
    N::ChainSpec: EthereumHardforks,
{
    pub fn from_reth(provider: RethRootProvider<N>) -> Self {
        Self { provider, limits: SelectionLimits::default() }
    }

    pub fn from_blockchain_provider(provider: BlockchainProvider<N>) -> Self {
        Self::from_reth(RethRootProvider::new(provider))
    }
}

impl<N> QueryHandler for QueryService<RethRootProvider<N>>
where
    N: ProviderNodeTypes<Primitives = EthPrimitives>,
    N::ChainSpec: EthereumHardforks,
{
    fn query_with_cancel(
        &self,
        request: QueryRequest,
        cancelled: &AtomicBool,
    ) -> Result<QueryResponse, QueryError> {
        validate_request(&request, self.limits, cancelled)?;
        if request.object != "receipts" {
            return Err(QueryError::UnsupportedObject);
        }
        let snapshot =
            self.provider.lookup(request.block_hash, ObjectKind::Receipts).map_err(|error| {
                match error {
                    RethRootProviderError::Lookup(error) => QueryError::Provider(error),
                    RethRootProviderError::Acquisition(error) => QueryError::Acquisition(error),
                }
            })?;
        query_snapshot_with_cancel(&request, &snapshot, self.limits, cancelled)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl QueryService<SourcePayloadSnapshot> {
    pub fn from_payload(snapshot: SourcePayloadSnapshot) -> Self {
        Self { provider: snapshot, limits: SelectionLimits::default() }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl QueryService<Arc<dyn ExecutionSourceProvider>> {
    pub fn from_execution_source(provider: Arc<dyn ExecutionSourceProvider>) -> Self {
        Self { provider, limits: SelectionLimits::default() }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl QueryHandler for QueryService<Arc<dyn ExecutionSourceProvider>> {
    fn transaction_receipt_with_cancel(
        &self,
        request: TransactionReceiptRequest,
        cancelled: &AtomicBool,
    ) -> Result<TransactionReceiptResponse, QueryError> {
        crate::association::check_request(&request, self.limits, cancelled)?;
        let source = self
            .provider
            .payload_source(request.block_hash.into(), cancelled)
            .map_err(QueryError::PayloadAcquisition)?;
        check_source(source.metadata(), request.block_hash, cancelled)?;
        crate::association::select_payload(
            source.payload().map_err(QueryError::PayloadAcquisition)?,
            &request,
            block_status(source.metadata().canonicality),
            self.limits,
            cancelled,
        )
        .map_err(Into::into)
    }

    fn query_with_cancel(
        &self,
        request: QueryRequest,
        cancelled: &AtomicBool,
    ) -> Result<QueryResponse, QueryError> {
        validate_request(&request, self.limits, cancelled)?;
        let source = if request.object == "receipts" {
            self.provider.receipt_source(request.block_hash.into(), cancelled)
        } else {
            self.provider.payload_source(request.block_hash.into(), cancelled)
        }
        .map_err(QueryError::PayloadAcquisition)?;
        check_source(source.metadata(), request.block_hash, cancelled)?;
        if request.object != "receipts" {
            return query_payload(
                &request,
                source.payload().map_err(QueryError::PayloadAcquisition)?,
                source.metadata(),
                self.limits,
                cancelled,
            );
        }
        let selection = select_with_cancel(
            source.receipt_snapshot(),
            &request.selection,
            self.limits,
            cancelled,
        )?;
        source_response(&request, source.metadata(), selection, self.limits)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn check_source(
    source: &PayloadSourceMetadata,
    block_hash: B256,
    cancelled: &AtomicBool,
) -> Result<(), QueryError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(SelectionError::Cancelled.into());
    }
    if source.native_block.hash != block_hash ||
        source.root_context != RootContext::RethExperimentalUnanchored
    {
        return Err(QueryError::InvalidSnapshot);
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn source_response(
    request: &QueryRequest,
    source: &PayloadSourceMetadata,
    selection: SelectionResponse,
    limits: SelectionLimits,
) -> Result<QueryResponse, QueryError> {
    let response = QueryResponse {
        block_hash: source.native_block.hash,
        object: request.object.clone(),
        root_context: "reth_experimental_unanchored".to_owned(),
        block_status: block_status(source.canonicality).to_owned(),
        selection,
    };
    encoded_size(&response, limits.response_bytes, "response bytes")?;
    Ok(response)
}

#[cfg(not(target_arch = "wasm32"))]
fn query_payload(
    request: &QueryRequest,
    payload: &ExecutionPayloadSnapshot,
    source: &PayloadSourceMetadata,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
) -> Result<QueryResponse, QueryError> {
    let selection = if let Some(object) = component_object(&request.object) {
        select_component_with_cancel(
            ComponentSnapshot::from_payload(payload, object),
            &request.selection,
            limits,
            cancelled,
        )?
        .selection
    } else {
        select_with_cancel(&payload.components().receipts, &request.selection, limits, cancelled)?
    };
    source_response(request, source, selection, limits)
}

#[cfg(not(target_arch = "wasm32"))]
impl QueryHandler for QueryService<SourcePayloadSnapshot> {
    fn transaction_receipt_with_cancel(
        &self,
        request: TransactionReceiptRequest,
        cancelled: &AtomicBool,
    ) -> Result<TransactionReceiptResponse, QueryError> {
        crate::association::associate(&self.provider, &request, self.limits, cancelled)
    }

    fn query_with_cancel(
        &self,
        request: QueryRequest,
        cancelled: &AtomicBool,
    ) -> Result<QueryResponse, QueryError> {
        validate_request(&request, self.limits, cancelled)?;
        let source = self.provider.source();
        if source.native_block.hash != request.block_hash {
            return Err(QueryError::Provider(LookupError::UnknownBlock));
        }
        if source.root_context != RootContext::RethExperimentalUnanchored {
            return Err(QueryError::InvalidSnapshot);
        }
        query_payload(&request, self.provider.payload(), source, self.limits, cancelled)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<N, B> QueryService<RethPayloadProvider<N, B>>
where
    N: ProviderNodeTypes<Primitives = EthPrimitives>,
    B: BalStore,
{
    pub fn from_payload_provider(
        provider: BlockchainProvider<N>,
        bal_store: B,
        limits: PayloadLimits,
    ) -> Self {
        Self {
            provider: RethPayloadProvider { provider, bal_store, limits },
            limits: SelectionLimits::default(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<N, B> QueryHandler for QueryService<RethPayloadProvider<N, B>>
where
    N: ProviderNodeTypes<Primitives = EthPrimitives>,
    N::ChainSpec: EthChainSpec<Header = Header> + EthereumHardforks,
    B: BalStore,
{
    fn transaction_receipt_with_cancel(
        &self,
        request: TransactionReceiptRequest,
        cancelled: &AtomicBool,
    ) -> Result<TransactionReceiptResponse, QueryError> {
        crate::association::check_request(&request, self.limits, cancelled)?;
        let snapshot = SourcePayloadSnapshot::from_reth_historical_with_cancel(
            &self.provider.provider,
            &self.provider.bal_store,
            request.block_hash,
            self.provider.limits,
            cancelled,
        )
        .map_err(QueryError::PayloadAcquisition)?;
        crate::association::associate(&snapshot, &request, self.limits, cancelled)
    }

    fn query_with_cancel(
        &self,
        request: QueryRequest,
        cancelled: &AtomicBool,
    ) -> Result<QueryResponse, QueryError> {
        validate_request(&request, self.limits, cancelled)?;
        if request.object == "receipts" {
            return QueryService::from_blockchain_provider(self.provider.provider.clone())
                .with_limits(self.limits)
                .query_with_cancel(request, cancelled);
        }
        let snapshot = SourcePayloadSnapshot::from_reth_historical_with_cancel(
            &self.provider.provider,
            &self.provider.bal_store,
            request.block_hash,
            self.provider.limits,
            cancelled,
        )
        .map_err(QueryError::PayloadAcquisition)?;
        QueryService::from_payload(snapshot)
            .with_limits(self.limits)
            .query_with_cancel(request, cancelled)
    }
}

fn component_object(object: &str) -> Option<ComponentObject> {
    match object {
        "transactions" => Some(ComponentObject::Transactions),
        "withdrawals" => Some(ComponentObject::Withdrawals),
        "execution_requests" => Some(ComponentObject::ExecutionRequests),
        "block_access_list" => Some(ComponentObject::BlockAccessList),
        "execution_summary" => Some(ComponentObject::ExecutionSummary),
        "execution_payload" => Some(ComponentObject::ExecutionPayload),
        _ => None,
    }
}

pub(crate) const fn block_status(status: CanonicalityStatus) -> &'static str {
    match status {
        CanonicalityStatus::Canonical => "canonical",
        CanonicalityStatus::NonCanonical => "non_canonical",
        CanonicalityStatus::Unknown => "unknown",
    }
}

fn validate_request(
    request: &QueryRequest,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
) -> Result<(), QueryError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(SelectionError::Cancelled.into());
    }
    if request.object != "receipts" && component_object(&request.object).is_none() {
        return Err(QueryError::UnsupportedObject);
    }
    encoded_size(request, limits.request_bytes, "request bytes")?;
    check_request(&request.selection, limits)?;
    if request.object == "execution_payload" &&
        request.selection.selections.iter().any(|selection| {
            selection.path != "." || !matches!(selection.operation, SelectionOperation::Whole {})
        })
    {
        return Err(SelectionError::UnsupportedOperation.into());
    }
    if request.object != "receipts" &&
        request
            .selection
            .selections
            .iter()
            .any(|selection| matches!(selection.operation, SelectionOperation::Filter { .. }))
    {
        return Err(SelectionError::UnsupportedOperation.into());
    }
    Ok(())
}

pub fn query_snapshot(
    request: &QueryRequest,
    snapshot: &ProviderSnapshot,
    limits: SelectionLimits,
) -> Result<QueryResponse, QueryError> {
    query_snapshot_with_cancel(request, snapshot, limits, &AtomicBool::new(false))
}

fn query_snapshot_with_cancel(
    request: &QueryRequest,
    snapshot: &ProviderSnapshot,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
) -> Result<QueryResponse, QueryError> {
    validate_request(request, limits, cancelled)?;
    if request.object != "receipts" {
        return Err(QueryError::UnsupportedObject);
    }
    if snapshot.block_hash() != request.block_hash ||
        snapshot.object() != ObjectKind::Receipts ||
        snapshot.root_context() != RootContext::RethExperimentalUnanchored
    {
        return Err(QueryError::InvalidSnapshot);
    }
    let selection =
        select_with_cancel(snapshot.receipt_snapshot(), &request.selection, limits, cancelled)?;
    if selection.root != snapshot.root() {
        return Err(QueryError::InvalidSnapshot);
    }
    let response = QueryResponse {
        block_hash: snapshot.block_hash(),
        object: request.object.clone(),
        root_context: "reth_experimental_unanchored".to_owned(),
        block_status: block_status(snapshot.block_status()).to_owned(),
        selection,
    };
    encoded_size(&response, limits.response_bytes, "response bytes")?;
    Ok(response)
}

pub fn verify_query_response(
    request: &QueryRequest,
    response: &QueryResponse,
    expected_root: B256,
    limits: SelectionLimits,
) -> Result<(), ResponseVerificationError> {
    if (request.object != "receipts" && component_object(&request.object).is_none()) ||
        response.object != request.object ||
        response.block_hash != request.block_hash ||
        response.root_context != "reth_experimental_unanchored" ||
        !matches!(response.block_status.as_str(), "canonical" | "non_canonical" | "unknown")
    {
        return Err(ResponseVerificationError::InvalidContext);
    }
    encoded_size(request, limits.request_bytes, "request bytes")
        .and_then(|_| encoded_size(response, limits.response_bytes, "response bytes"))
        .map_err(ResponseVerificationError::Selection)?;
    if let Some(object) = component_object(&request.object) {
        verify_component_response(
            object,
            &request.selection,
            &response.selection,
            expected_root,
            limits,
        )
    } else {
        verify_receipt_selection(&request.selection, &response.selection, expected_root, limits)
    }
    .map_err(ResponseVerificationError::Selection)
}

#[cfg(test)]
mod tests;
