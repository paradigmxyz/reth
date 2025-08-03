//! Validates execution payload wrt Ethereum Execution Engine API version.

use alloy_eips::Decodable2718;
use alloy_primitives::{Bytes, U256};
use alloy_rpc_types_engine::{ExecutionData, PayloadError};
pub use alloy_rpc_types_engine::{
    ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4,
    ExecutionPayloadV1, PayloadAttributes as EthPayloadAttributes,
};
use reth_chainspec::{EthChainSpec, EthereumHardforks, Hardforks};
use reth_engine_primitives::{EngineValidator, PayloadValidator};
use reth_ethereum_payload_builder::EthereumExecutionPayloadValidator;
use reth_ethereum_primitives::{Block, EthPrimitives};
use reth_evm::{
    block::BlockExecutorFactory, eth::EthBlockExecutionCtx, ConfigureEvm, EvmEnv, EvmFactory,
};
use reth_evm_ethereum::revm_spec_by_timestamp_and_block_number;
use reth_node_api::{EvmPayloadValidator, PayloadTypes};
use reth_payload_primitives::{
    validate_execution_requests, validate_version_specific_fields, EngineApiMessageVersion,
    EngineObjectValidationError, NewPayloadError, PayloadOrAttributes,
};
use reth_primitives_traits::{RecoveredBlock, SignedTransaction, TxTy};
use revm::{
    context::{BlockEnv, CfgEnv},
    context_interface::block::BlobExcessGasAndPrice,
    primitives::hardfork::SpecId,
};
use std::{
    collections::BTreeMap,
    sync::{mpsc, Arc},
};

/// Validator for the ethereum engine API.
#[derive(Debug, Clone)]
pub struct EthereumEngineValidator<ChainSpec = reth_chainspec::ChainSpec> {
    inner: EthereumExecutionPayloadValidator<ChainSpec>,
}

impl<ChainSpec> EthereumEngineValidator<ChainSpec> {
    /// Instantiates a new validator.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { inner: EthereumExecutionPayloadValidator::new(chain_spec) }
    }

    /// Returns the chain spec used by the validator.
    #[inline]
    fn chain_spec(&self) -> &ChainSpec {
        self.inner.chain_spec()
    }
}

impl<ChainSpec, Types> PayloadValidator<Types> for EthereumEngineValidator<ChainSpec>
where
    ChainSpec: EthChainSpec + EthereumHardforks + 'static,
    Types: PayloadTypes<ExecutionData = ExecutionData>,
{
    type Block = Block;

    fn ensure_well_formed_payload(
        &self,
        payload: ExecutionData,
    ) -> Result<RecoveredBlock<Self::Block>, NewPayloadError> {
        let sealed_block = self.inner.ensure_well_formed_payload(payload)?;
        sealed_block.try_recover().map_err(|e| NewPayloadError::Other(e.into()))
    }
}

impl<ChainSpec, Types, Evm> EvmPayloadValidator<Types, Evm> for EthereumEngineValidator<ChainSpec>
where
    ChainSpec: Hardforks + EthChainSpec + EthereumHardforks + 'static,
    Types: PayloadTypes<ExecutionData = ExecutionData>,
    Evm: ConfigureEvm<
        Primitives = EthPrimitives,
        BlockExecutorFactory: for<'a> BlockExecutorFactory<
            EvmFactory: EvmFactory<Spec = SpecId>,
            ExecutionCtx<'a> = EthBlockExecutionCtx<'a>,
        >,
    >,
{
    fn evm_env_for_payload(
        &self,
        payload: &ExecutionData,
        _evm: &Evm,
    ) -> Result<reth_evm::EvmEnvFor<Evm>, NewPayloadError> {
        let timestamp = payload.payload.timestamp();
        let block_number = payload.payload.block_number();

        let blob_params = self.chain_spec().blob_params_at_timestamp(timestamp);
        let spec =
            revm_spec_by_timestamp_and_block_number(self.chain_spec(), timestamp, block_number);

        // configure evm env based on parent block
        let mut cfg_env =
            CfgEnv::new().with_chain_id(self.chain_spec().chain().id()).with_spec(spec);

        if let Some(blob_params) = &blob_params {
            cfg_env.set_max_blobs_per_tx(blob_params.max_blobs_per_tx);
        }

        // derive the EIP-4844 blob fees from the header's `excess_blob_gas` and the current
        // blobparams
        let blob_excess_gas_and_price =
            payload.payload.as_v3().map(|v3| v3.excess_blob_gas).zip(blob_params).map(
                |(excess_blob_gas, params)| {
                    let blob_gasprice = params.calc_blob_fee(excess_blob_gas);
                    BlobExcessGasAndPrice { excess_blob_gas, blob_gasprice }
                },
            );

        let block_env = BlockEnv {
            number: U256::from(block_number),
            beneficiary: payload.payload.as_v1().fee_recipient,
            timestamp: U256::from(timestamp),
            difficulty: if spec >= SpecId::MERGE {
                U256::ZERO
            } else {
                payload.payload.as_v1().prev_randao.into()
            },
            prevrandao: (spec >= SpecId::MERGE).then(|| payload.payload.as_v1().prev_randao),
            gas_limit: payload.payload.as_v1().gas_limit,
            basefee: payload.payload.as_v1().base_fee_per_gas.to(),
            blob_excess_gas_and_price,
        };

        Ok(EvmEnv { cfg_env, block_env })
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<impl reth_node_api::ExecutableTxIterator<Evm>, NewPayloadError> {
        let transactions = payload.payload.transactions().clone();
        let (iter_tx, from_iter) = mpsc::channel();
        rayon::spawn(move || {
            let (tasks_tx, from_tasks) = mpsc::channel();
            let mut tasks = Vec::new();
            let num_tasks = 10;

            for (i, tx) in transactions.into_iter().enumerate() {
                let task_idx = i % num_tasks;
                if task_idx >= tasks.len() {
                    let (to_task, task_rx) = mpsc::channel();
                    let task_tx = tasks_tx.clone();
                    rayon::spawn(move || {
                        while let Ok((idx, tx)) = task_rx.recv() {
                            let decode = |tx: Bytes| -> Result<_, _> {
                                let tx = TxTy::<Evm::Primitives>::decode_2718_exact(tx.as_ref())
                                    .map_err(alloy_rlp::Error::from)
                                    .map_err(PayloadError::from)?;
                                let signer = tx.try_recover().map_err(NewPayloadError::other)?;
                                Ok::<_, NewPayloadError>(tx.with_signer(signer))
                            };

                            let _ = task_tx.send((idx, decode(tx)));
                        }
                    });

                    tasks.push(to_task);
                }

                let _ = tasks[task_idx].send((i, tx));
            }

            drop(tasks_tx);
            drop(tasks);

            let mut next_result_idx = 0;
            let mut buffered = BTreeMap::new();
            while let Ok((idx, result)) = from_tasks.recv() {
                if next_result_idx == idx {
                    let _ = iter_tx.send(result);
                    next_result_idx += 1;

                    while let Some(result) = buffered.remove(&next_result_idx) {
                        let _ = iter_tx.send(result);
                        next_result_idx += 1;
                    }
                } else {
                    buffered.insert(idx, result);
                }
            }
        });

        Ok(from_iter.into_iter())
    }

    fn execution_ctx_for_payload<'a>(
        &self,
        payload: &'a ExecutionData,
    ) -> Result<impl reth_node_api::ExecutionCtxProvider<Evm> + 'a, NewPayloadError> {
        Ok(payload)
    }
}

impl<ChainSpec, Types> EngineValidator<Types> for EthereumEngineValidator<ChainSpec>
where
    ChainSpec: EthChainSpec + EthereumHardforks + 'static,
    Types: PayloadTypes<PayloadAttributes = EthPayloadAttributes, ExecutionData = ExecutionData>,
{
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, Types::ExecutionData, EthPayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        payload_or_attrs
            .execution_requests()
            .map(|requests| validate_execution_requests(requests))
            .transpose()?;

        validate_version_specific_fields(self.chain_spec(), version, payload_or_attrs)
    }

    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attributes: &EthPayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        validate_version_specific_fields(
            self.chain_spec(),
            version,
            PayloadOrAttributes::<Types::ExecutionData, EthPayloadAttributes>::PayloadAttributes(
                attributes,
            ),
        )
    }
}
