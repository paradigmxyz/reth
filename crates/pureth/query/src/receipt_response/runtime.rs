use super::{
    extend_indexed_receipt_responses,
    native::{convert_typed_with_source, ConversionBudget},
    ReceiptProofSupport, ReceiptResponse, WholeReceiptProof,
};
use crate::{selection::encoded_size, SelectionError, SelectionLimits};
use alloy_consensus::{BlockHeader, Eip658Value, ReceiptEnvelope, TxReceipt};
use alloy_eips::BlockId;
use alloy_primitives::{Address, Bloom, Sealable, U256, U64};
use alloy_rpc_types_eth::{Log, TransactionReceipt};
use reth_chainspec::EthChainSpec;
use reth_primitives_traits::{BlockBody, SealedBlock, SealedHeader};
use reth_pureth_block::{
    ExecutionBlock, ExecutionForkConfig, ExecutionHeader, ExecutionPrimitives, ExecutionSource,
    ExecutionSourceProvider, PayloadSourceError,
};
use reth_pureth_receipt::RootContext;
use reth_rpc_convert::{
    transaction::{ConvertReceiptInput, HeaderConverter, ReceiptConverter},
    RpcTypes,
};
use reth_rpc_eth_types::{receipt::EthReceiptConverter, EthApiError};
use serde::{Deserialize, Deserializer, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub type ExecutionRpcReceipt = TransactionReceipt<ExecutionReceiptEnvelope>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExecutionReceiptEnvelope {
    #[serde(flatten)]
    receipt: ExecutionReceiptKind,
    #[serde(rename = "sszProof", skip_serializing_if = "Option::is_none")]
    proof: Option<Option<WholeReceiptProof>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
enum ExecutionReceiptKind {
    Legacy(Box<ReceiptEnvelope<Log>>),
    Selected(SelectedReceipt),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SelectedReceipt {
    #[serde(rename = "type")]
    transaction_type: U64,
    status: U64,
    cumulative_gas_used: U64,
    logs: Vec<Log>,
    authorities: Option<Vec<Address>>,
}

impl ExecutionReceiptEnvelope {
    pub(super) const fn selected(
        transaction_type: U64,
        status: U64,
        cumulative_gas_used: U64,
        logs: Vec<Log>,
        authorities: Option<Vec<Address>>,
    ) -> Self {
        Self {
            receipt: ExecutionReceiptKind::Selected(SelectedReceipt {
                transaction_type,
                status,
                cumulative_gas_used,
                logs,
                authorities,
            }),
            proof: None,
        }
    }

    fn legacy(receipt: ReceiptEnvelope<Log>) -> Self {
        Self { receipt: ExecutionReceiptKind::Legacy(Box::new(receipt)), proof: None }
    }
}

impl<'de> Deserialize<'de> for ExecutionReceiptEnvelope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
        let proof = fields
            .remove("sszProof")
            .map(serde_json::from_value::<Option<WholeReceiptProof>>)
            .transpose()
            .map_err(serde::de::Error::custom)?;
        let receipt = if fields.contains_key("authorities") {
            let receipt: SelectedReceipt =
                serde_json::from_value(serde_json::Value::Object(fields))
                    .map_err(serde::de::Error::custom)?;
            if receipt.transaction_type > U64::from(4) ||
                receipt.status > U64::from(1) ||
                (receipt.transaction_type == U64::from(4)) != receipt.authorities.is_some()
            {
                return Err(serde::de::Error::custom("invalid selected receipt fields"));
            }
            ExecutionReceiptKind::Selected(receipt)
        } else {
            ExecutionReceiptKind::Legacy(Box::new(
                serde_json::from_value(serde_json::Value::Object(fields))
                    .map_err(serde::de::Error::custom)?,
            ))
        };
        Ok(Self { receipt, proof })
    }
}

impl TxReceipt for ExecutionReceiptEnvelope {
    type Log = Log;

    fn status_or_post_state(&self) -> Eip658Value {
        match &self.receipt {
            ExecutionReceiptKind::Legacy(receipt) => receipt.status_or_post_state(),
            ExecutionReceiptKind::Selected(receipt) => {
                Eip658Value::Eip658(receipt.status == U64::from(1))
            }
        }
    }

    fn status(&self) -> bool {
        self.status_or_post_state().coerce_status()
    }

    fn bloom(&self) -> Bloom {
        match &self.receipt {
            ExecutionReceiptKind::Legacy(receipt) => receipt.bloom(),
            ExecutionReceiptKind::Selected(receipt) => {
                receipt.logs.iter().map(|log| &log.inner).collect()
            }
        }
    }

    fn cumulative_gas_used(&self) -> u64 {
        match &self.receipt {
            ExecutionReceiptKind::Legacy(receipt) => receipt.cumulative_gas_used(),
            ExecutionReceiptKind::Selected(receipt) => receipt.cumulative_gas_used.to(),
        }
    }

    fn logs(&self) -> &[Log] {
        match &self.receipt {
            ExecutionReceiptKind::Legacy(receipt) => receipt.logs(),
            ExecutionReceiptKind::Selected(receipt) => &receipt.logs,
        }
    }

    fn into_logs(self) -> Vec<Log> {
        match self.receipt {
            ExecutionReceiptKind::Legacy(receipt) => (*receipt).into_logs(),
            ExecutionReceiptKind::Selected(receipt) => receipt.logs,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ExecutionRpcTypes;

impl RpcTypes for ExecutionRpcTypes {
    type Header = alloy_rpc_types_eth::Header;
    type Receipt = ExecutionRpcReceipt;
    type Log = Log;
    type TransactionResponse = alloy_rpc_types_eth::Transaction;
    type TransactionRequest = alloy_rpc_types_eth::TransactionRequest;
}

#[derive(Debug)]
pub struct ExecutionReceiptConverter<S> {
    native: EthReceiptConverter<S>,
    chain_spec: Arc<S>,
    fork: ExecutionForkConfig,
    source: Option<Arc<ExecutionSource>>,
    provider: Option<Arc<dyn ExecutionSourceProvider>>,
    limits: SelectionLimits,
    proofs: bool,
}

impl<S> Clone for ExecutionReceiptConverter<S> {
    fn clone(&self) -> Self {
        Self {
            native: self.native.clone(),
            chain_spec: self.chain_spec.clone(),
            fork: self.fork,
            source: self.source.clone(),
            provider: self.provider.clone(),
            limits: self.limits,
            proofs: self.proofs,
        }
    }
}

impl<S: EthChainSpec + 'static> ExecutionReceiptConverter<S> {
    pub fn new(chain_spec: Arc<S>, fork: ExecutionForkConfig, limits: SelectionLimits) -> Self {
        Self {
            native: EthReceiptConverter::new(chain_spec.clone()),
            chain_spec,
            fork,
            source: None,
            provider: None,
            limits,
            proofs: false,
        }
    }

    pub fn with_source(mut self, source: Arc<ExecutionSource>) -> Result<Self, EthApiError> {
        if source.chain_id() != self.chain_spec.chain().id() ||
            source.metadata().root_context != RootContext::RethExperimentalUnanchored ||
            source.block().header().is_ssz() !=
                self.fork.is_active_at_timestamp(source.block().timestamp())
        {
            return Err(EthApiError::InternalEthError);
        }
        self.source = Some(source);
        Ok(self)
    }

    pub fn with_source_provider(mut self, provider: Arc<dyn ExecutionSourceProvider>) -> Self {
        self.provider = Some(provider);
        self.source = None;
        self
    }

    pub const fn with_receipt_proof(mut self, enabled: bool) -> Self {
        self.proofs = enabled;
        self
    }

    fn attach_proofs(
        &self,
        receipts: Vec<ExecutionRpcReceipt>,
        indices: &[u64],
        cancelled: &AtomicBool,
    ) -> Result<Vec<ExecutionRpcReceipt>, EthApiError> {
        let support = match &self.source {
            Some(source) if source.block().header().is_ssz() => ReceiptProofSupport::Retained {
                block_hash: source.block().hash(),
                snapshot: source.receipt_snapshot(),
            },
            _ => ReceiptProofSupport::Unsupported,
        };
        let receipts = extend_indexed_receipt_responses(
            receipts,
            indices,
            support,
            self.proofs,
            self.limits,
            cancelled,
        )
        .map_err(selection_error)?;
        Ok(receipts
            .into_iter()
            .map(|receipt| match receipt {
                ReceiptResponse::Legacy(receipt) => receipt,
                ReceiptResponse::WithProof { mut receipt, ssz_proof } => {
                    receipt.inner.proof = Some(ssz_proof);
                    receipt
                }
            })
            .collect())
    }

    pub fn convert_receipts_with_cancel(
        &self,
        inputs: Vec<ConvertReceiptInput<'_, ExecutionPrimitives>>,
        cancelled: &AtomicBool,
    ) -> Result<Vec<ExecutionRpcReceipt>, EthApiError> {
        if cancelled.load(Ordering::Relaxed) || reth_tasks::cancel::is_cancelled() {
            return Err(selection_error(SelectionError::Cancelled));
        }
        let Some(first) = inputs.first() else {
            let receipts = Vec::new();
            encoded_size(&receipts, self.limits.response_bytes, "response bytes")
                .map_err(selection_error)?;
            return Ok(receipts);
        };
        let active = self.fork.is_active_at_timestamp(first.meta.timestamp);
        if active && let Some(provider) = &self.provider {
            let source = provider.receipt_source(first.meta.block_hash.into(), cancelled).map_err(
                |error| {
                    EthApiError::other(crate::rpc::rpc_error(
                        crate::QueryError::PayloadAcquisition(error),
                    ))
                },
            )?;
            let mut converter = self.clone().with_source(source)?;
            converter.provider = None;
            return converter.convert_receipts_with_cancel(inputs, cancelled);
        }
        let identity = (first.meta.block_hash, first.meta.block_number, first.meta.timestamp);
        if inputs.iter().any(|input| {
            (input.meta.block_hash, input.meta.block_number, input.meta.timestamp) != identity
        }) {
            return Err(EthApiError::InternalEthError);
        }
        if self.source.as_ref().is_some_and(|source| {
            identity != (source.block().hash(), source.block().number(), source.block().timestamp())
        }) {
            return Err(EthApiError::InternalEthError);
        }
        if !active {
            let indices = inputs.iter().map(|input| input.meta.index).collect::<Vec<_>>();
            let receipts = <EthReceiptConverter<S> as ReceiptConverter<ExecutionPrimitives>>::convert_receipts(&self.native, inputs)?;
            let receipts = receipts
                .into_iter()
                .map(|receipt| receipt.map_inner(ExecutionReceiptEnvelope::legacy))
                .collect::<Vec<_>>();
            encoded_size(&receipts, self.limits.response_bytes, "response bytes")
                .map_err(selection_error)?;
            return self.attach_proofs(receipts, &indices, cancelled);
        }
        let source = self.source.as_ref().ok_or(EthApiError::InternalEthError)?;
        let block = source.block();
        let snapshot = source.receipt_snapshot();
        let mut indices = Vec::with_capacity(inputs.len());
        for input in &inputs {
            if cancelled.load(Ordering::Relaxed) || reth_tasks::cancel::is_cancelled() {
                return Err(selection_error(SelectionError::Cancelled));
            }
            let index =
                usize::try_from(input.meta.index).map_err(|_| EthApiError::InternalEthError)?;
            if identity != (block.hash(), block.number(), block.timestamp()) ||
                source.native_receipts().get(index) != Some(&input.receipt) ||
                block.body().transactions().get(index) != Some(*input.tx.inner()) ||
                block.senders().get(index) != Some(&input.tx.signer()) ||
                input.meta.tx_hash != *input.tx.inner().hash() ||
                input.meta.base_fee != block.base_fee_per_gas() ||
                input.meta.excess_blob_gas != block.excess_blob_gas()
            {
                return Err(EthApiError::InternalEthError);
            }
            indices.push(input.meta.index);
        }
        let receipts =
            <EthReceiptConverter<S> as ReceiptConverter<ExecutionPrimitives>>::convert_receipts(
                &self.native,
                inputs,
            )?;
        let mut budget = ConversionBudget::for_batch(&receipts, snapshot, self.limits, cancelled)
            .map_err(selection_error)?;
        let receipts = receipts
            .into_iter()
            .zip(indices.iter().copied())
            .map(|(receipt, index)| {
                convert_typed_with_source(
                    receipt,
                    block,
                    snapshot,
                    index,
                    self.limits,
                    cancelled,
                    &mut budget,
                )
                .map_err(selection_error)
            })
            .collect::<Result<Vec<_>, _>>()?;
        encoded_size(&receipts, self.limits.response_bytes, "response bytes")
            .map_err(selection_error)?;
        let receipts = self.attach_proofs(receipts, &indices, cancelled)?;
        if cancelled.load(Ordering::Relaxed) || reth_tasks::cancel::is_cancelled() {
            return Err(selection_error(SelectionError::Cancelled));
        }
        Ok(receipts)
    }
}

impl<S: EthChainSpec + 'static> ReceiptConverter<ExecutionPrimitives>
    for ExecutionReceiptConverter<S>
{
    type RpcReceipt = ExecutionRpcReceipt;
    type RpcLog = Log;
    type Error = EthApiError;

    fn convert_log(
        &self,
        log: Log,
        _receipt: &reth_ethereum_primitives::Receipt,
        header: &SealedHeader<ExecutionHeader>,
    ) -> Result<Log, EthApiError> {
        if header.is_ssz() ||
            self.fork.is_active_at_timestamp(header.timestamp()) ||
            header.hash() != header.hash_slow()
        {
            return Err(EthApiError::InternalEthError);
        }
        Ok(log)
    }

    fn convert_log_with_index(
        &self,
        mut log: Log,
        receipt: &reth_ethereum_primitives::Receipt,
        header: &SealedHeader<ExecutionHeader>,
        receipt_log_index: usize,
    ) -> Result<Log, EthApiError> {
        let active = self.fork.is_active_at_timestamp(header.timestamp());
        if active != header.is_ssz() ||
            header.hash() != header.hash_slow() ||
            receipt.logs().get(receipt_log_index) != Some(&log.inner) ||
            log.block_hash != Some(header.hash()) ||
            log.block_number != Some(header.number())
        {
            return Err(EthApiError::InternalEthError);
        }
        if active {
            log.log_index = Some(receipt_log_index as u64);
        }
        Ok(log)
    }

    fn convert_receipts(
        &self,
        inputs: Vec<ConvertReceiptInput<'_, ExecutionPrimitives>>,
    ) -> Result<Vec<ExecutionRpcReceipt>, EthApiError> {
        self.convert_receipts_with_cancel(inputs, &AtomicBool::new(false))
    }

    fn convert_receipts_with_block(
        &self,
        inputs: Vec<ConvertReceiptInput<'_, ExecutionPrimitives>>,
        block: &SealedBlock<ExecutionBlock>,
    ) -> Result<Vec<ExecutionRpcReceipt>, EthApiError> {
        self.convert_receipts_with_context(inputs, block, false)
    }

    fn convert_receipts_with_context(
        &self,
        inputs: Vec<ConvertReceiptInput<'_, ExecutionPrimitives>>,
        block: &SealedBlock<ExecutionBlock>,
        require_canonical: bool,
    ) -> Result<Vec<ExecutionRpcReceipt>, EthApiError> {
        if block.header().is_ssz() != self.fork.is_active_at_timestamp(block.timestamp()) ||
            block.header().hash_slow() != block.hash()
        {
            return Err(EthApiError::InternalEthError);
        }
        if block.header().is_ssz() &&
            let Some(provider) = &self.provider
        {
            let selector = if require_canonical {
                BlockId::hash_canonical(block.hash())
            } else {
                block.hash().into()
            };
            let source =
                provider.receipt_source(selector, &AtomicBool::new(false)).map_err(|error| {
                    EthApiError::other(crate::rpc::rpc_error(
                        crate::QueryError::PayloadAcquisition(error),
                    ))
                })?;
            let mut converter = self.clone().with_source(source)?;
            converter.provider = None;
            return converter.convert_receipts_with_context(inputs, block, require_canonical);
        }
        if require_canonical &&
            block.header().is_ssz() &&
            self.source.as_ref().is_some_and(|source| {
                source.metadata().canonicality != reth_pureth_receipt::CanonicalityStatus::Canonical
            })
        {
            return Err(EthApiError::other(crate::rpc::rpc_error(
                crate::QueryError::PayloadAcquisition(PayloadSourceError::NonCanonicalBlock(
                    block.num_hash(),
                )),
            )));
        }
        if block.header().is_ssz() && self.source.is_none() ||
            self.source.as_ref().is_some_and(|source| source.block().hash() != block.hash()) ||
            inputs.iter().any(|input| {
                input.meta.block_hash != block.hash() ||
                    input.meta.timestamp != block.timestamp() ||
                    input.meta.block_number != block.number()
            })
        {
            return Err(EthApiError::InternalEthError);
        }
        self.convert_receipts(inputs)
    }
}

fn selection_error(error: SelectionError) -> EthApiError {
    match error {
        SelectionError::Cancelled | SelectionError::LimitExceeded(_) => {
            EthApiError::other(crate::rpc::rpc_error(crate::QueryError::Selection(error)))
        }
        _ => EthApiError::InternalEthError,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ExecutionHeaderConverter {
    fork: ExecutionForkConfig,
}

impl ExecutionHeaderConverter {
    pub const fn new(fork: ExecutionForkConfig) -> Self {
        Self { fork }
    }
}

impl HeaderConverter<ExecutionHeader, alloy_rpc_types_eth::Header> for ExecutionHeaderConverter {
    type Err = EthApiError;

    fn convert_header(
        &self,
        header: SealedHeader<ExecutionHeader>,
        block_size: Option<usize>,
    ) -> Result<alloy_rpc_types_eth::Header, EthApiError> {
        if header.is_ssz() != self.fork.is_active_at_timestamp(header.timestamp()) ||
            header.hash() != header.hash_slow()
        {
            return Err(EthApiError::InternalEthError);
        }
        Ok(alloy_rpc_types_eth::Header::from_consensus(
            SealedHeader::new(header.as_native().clone(), header.hash()).into(),
            None,
            block_size.map(U256::from),
        ))
    }
}
