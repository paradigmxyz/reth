use crate::{
    selection::{check_request, encoded_size, select_with_cancel},
    verify_receipt_selection, SelectionError, SelectionLimits, SelectionRequest, SelectionResponse,
};
use alloy_primitives::B256;
use reth_chainspec::EthereumHardforks;
use reth_ethereum_primitives::EthPrimitives;
use reth_provider::providers::{BlockchainProvider, ProviderNodeTypes};
use reth_pureth_receipt::{
    CanonicalityStatus, DeterministicProvider, HistoricalAcquisitionError, LookupError, ObjectKind,
    ProviderBuildError, ProviderSnapshot, RethRootProvider, RethRootProviderError, RootContext,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

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
pub struct QueryService<P = DeterministicProvider> {
    provider: P,
    limits: SelectionLimits,
}

pub trait QueryHandler {
    fn query_with_cancel(
        &self,
        request: QueryRequest,
        cancelled: &AtomicBool,
    ) -> Result<QueryResponse, QueryError>;

    fn query(&self, request: QueryRequest) -> Result<QueryResponse, QueryError> {
        self.query_with_cancel(request, &AtomicBool::new(false))
    }
}

impl QueryService<DeterministicProvider> {
    pub fn new() -> Result<Self, ProviderBuildError> {
        Ok(Self::from_provider(DeterministicProvider::new()?))
    }

    pub fn from_provider(provider: DeterministicProvider) -> Self {
        Self { provider, limits: SelectionLimits::default() }
    }
}

impl<P> QueryService<P> {
    pub const fn with_limits(mut self, limits: SelectionLimits) -> Self {
        self.limits = limits;
        self
    }
}

impl QueryHandler for QueryService<DeterministicProvider> {
    fn query_with_cancel(
        &self,
        request: QueryRequest,
        cancelled: &AtomicBool,
    ) -> Result<QueryResponse, QueryError> {
        validate_request(&request, self.limits, cancelled)?;
        let snapshot = self
            .provider
            .lookup(request.block_hash, ObjectKind::Receipts)
            .map_err(QueryError::Provider)?;
        query_snapshot_with_cancel(&request, snapshot, self.limits, cancelled)
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

fn validate_request(
    request: &QueryRequest,
    limits: SelectionLimits,
    cancelled: &AtomicBool,
) -> Result<(), QueryError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(SelectionError::Cancelled.into());
    }
    if request.object != "receipts" {
        return Err(QueryError::UnsupportedObject);
    }
    encoded_size(request, limits.request_bytes, "request bytes")?;
    check_request(&request.selection, limits)?;
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
    if snapshot.block_hash() != request.block_hash || snapshot.object() != ObjectKind::Receipts {
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
        root_context: match snapshot.root_context() {
            RootContext::DeterministicTestData => "deterministic_test_data",
            RootContext::RethExperimentalUnanchored => "reth_experimental_unanchored",
        }
        .to_owned(),
        block_status: match snapshot.block_status() {
            CanonicalityStatus::Canonical => "canonical",
            CanonicalityStatus::NonCanonical => "non_canonical",
            CanonicalityStatus::Unknown => "unknown",
        }
        .to_owned(),
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
    if request.object != "receipts" ||
        response.object != request.object ||
        response.block_hash != request.block_hash ||
        !matches!(
            response.root_context.as_str(),
            "deterministic_test_data" | "reth_experimental_unanchored"
        ) ||
        !matches!(response.block_status.as_str(), "canonical" | "non_canonical" | "unknown")
    {
        return Err(ResponseVerificationError::InvalidContext);
    }
    encoded_size(request, limits.request_bytes, "request bytes")
        .and_then(|_| encoded_size(response, limits.response_bytes, "response bytes"))
        .map_err(ResponseVerificationError::Selection)?;
    verify_receipt_selection(&request.selection, &response.selection, expected_root, limits)
        .map_err(ResponseVerificationError::Selection)
}

#[cfg(test)]
mod tests;
