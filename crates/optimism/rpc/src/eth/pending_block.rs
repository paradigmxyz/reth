//! Loads OP pending block for a RPC response.

use std::sync::Arc;

use crate::{OpEthApi, OpEthApiError};
use alloy_consensus::{BlockHeader, Transaction};
use alloy_eips::eip7840::BlobParams;
use alloy_json_rpc::{Id, Request, SerializedRequest};
use alloy_pubsub::PubSubConnect;
use alloy_rpc_client::WsConnect;
use alloy_rpc_types_mev::FlashBlock;
use jsonrpsee_core::Cow;
use reqwest::Url;
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_errors::{BlockExecutionError, BlockValidationError, RethError};
use reth_evm::{
    execute::{BlockBuilder, BlockBuilderOutcome, ExecutionOutcome},
    ConfigureEvm, Evm,
};
use reth_primitives_traits::{transaction::error::InvalidTransactionError, RecoveredBlock};
use reth_revm::{database::StateProviderDatabase, State};
use reth_rpc_eth_api::{
    helpers::{pending_block::PendingEnvBuilder, LoadPendingBlock},
    FromEthApiError, FromEvmError, RpcConvert, RpcNodeCore,
};
use reth_rpc_eth_types::{EthApiError, PendingBlock, PendingBlockEnvOrigin};
use reth_storage_api::{ProviderBlock, ProviderReceipt, ReceiptProvider, StateProviderFactory};
use reth_transaction_pool::{
    error::InvalidPoolTransactionError, BestTransactionsAttributes, TransactionPool,
};
use revm::context::Block;
use serde_json::Value;
use tokio::time::Duration;

pub(super) type SharedFlashblockState = Arc<tokio::sync::RwLock<FlashBlock>>;

impl<N, Rpc> LoadPendingBlock for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    #[inline]
    fn pending_block(&self) -> &tokio::sync::Mutex<Option<PendingBlock<N::Primitives>>> {
        self.inner.eth_api.pending_block()
    }

    #[inline]
    fn pending_env_builder(&self) -> &dyn PendingEnvBuilder<Self::Evm> {
        self.inner.eth_api.pending_env_builder()
    }

    /// Returns the locally built pending block
    async fn local_pending_block(
        &self,
    ) -> Result<
        Option<(
            Arc<RecoveredBlock<ProviderBlock<Self::Provider>>>,
            Arc<Vec<ProviderReceipt<Self::Provider>>>,
        )>,
        Self::Error,
    > {
        // See: <https://github.com/ethereum-optimism/op-geth/blob/f2e69450c6eec9c35d56af91389a1c47737206ca/miner/worker.go#L367-L375>
        let pending = self.pending_block_env_and_cfg()?;
        let parent = match pending.origin {
            PendingBlockEnvOrigin::ActualPending(block, receipts) => {
                return Ok(Some((block, receipts)));
            }
            PendingBlockEnvOrigin::DerivedFromLatest(parent) => parent,
        };
        let state_provider = self
            .provider()
            .state_by_block_hash(parent.hash())
            .map_err(Self::Error::from_eth_err)?;
        let state = StateProviderDatabase::new(&state_provider);
        let mut db = State::builder().with_database(state).with_bundle_update().build();

        let mut builder = self
            .evm_config()
            .builder_for_next_block(&mut db, &parent, self.next_env_attributes(&parent)?)
            .map_err(RethError::other)
            .map_err(Self::Error::from_eth_err)?;

        builder.apply_pre_execution_changes().map_err(Self::Error::from_eth_err)?;

        let block_env = builder.evm_mut().block().clone();

        let blob_params = self
            .provider()
            .chain_spec()
            .blob_params_at_timestamp(parent.timestamp())
            .unwrap_or_else(BlobParams::cancun);
        let mut cumulative_gas_used = 0;
        let mut sum_blob_gas_used = 0;
        let block_gas_limit: u64 = block_env.gas_limit;

        let mut best_txs =
            self.pool().best_transactions_with_attributes(BestTransactionsAttributes::new(
                block_env.basefee,
                block_env.blob_gasprice().map(|gasprice| gasprice as u64),
            ));

        while let Some(pool_tx) = best_txs.next() {
            // ensure we still have capacity for this transaction
            if cumulative_gas_used + pool_tx.gas_limit() > block_gas_limit {
                // we can't fit this transaction into the block, so we need to mark it as invalid
                // which also removes all dependent transaction from the iterator before we can
                // continue
                best_txs.mark_invalid(
                    &pool_tx,
                    InvalidPoolTransactionError::ExceedsGasLimit(
                        pool_tx.gas_limit(),
                        block_gas_limit,
                    ),
                );
                continue
            }

            if pool_tx.origin.is_private() {
                // we don't want to leak any state changes made by private transactions, so we mark
                // them as invalid here which removes all dependent transactions from the iterator
                // before we can continue
                best_txs.mark_invalid(
                    &pool_tx,
                    InvalidPoolTransactionError::Consensus(
                        InvalidTransactionError::TxTypeNotSupported,
                    ),
                );
                continue
            }

            // convert tx to a signed transaction
            let tx = pool_tx.to_consensus();

            // There's only limited amount of blob space available per block, so we need to check if
            // the EIP-4844 can still fit in the block
            if let Some(tx_blob_gas) = tx.blob_gas_used() {
                if sum_blob_gas_used + tx_blob_gas > blob_params.max_blob_gas_per_block() {
                    // we can't fit this _blob_ transaction into the block, so we mark it as
                    // invalid, which removes its dependent transactions from
                    // the iterator. This is similar to the gas limit condition
                    // for regular transactions above.
                    best_txs.mark_invalid(
                        &pool_tx,
                        InvalidPoolTransactionError::ExceedsGasLimit(
                            tx_blob_gas,
                            blob_params.max_blob_gas_per_block(),
                        ),
                    );
                    continue
                }
            }

            let gas_used = match builder.execute_transaction(tx.clone()) {
                Ok(gas_used) => gas_used,
                Err(BlockExecutionError::Validation(BlockValidationError::InvalidTx {
                    error,
                    ..
                })) => {
                    if error.is_nonce_too_low() {
                        // if the nonce is too low, we can skip this transaction
                    } else {
                        // if the transaction is invalid, we can skip it and all of its
                        // descendants
                        best_txs.mark_invalid(
                            &pool_tx,
                            InvalidPoolTransactionError::Consensus(
                                InvalidTransactionError::TxTypeNotSupported,
                            ),
                        );
                    }
                    continue
                }
                // this is an error that we should treat as fatal for this attempt
                Err(err) => return Err(Self::Error::from_eth_err(err)),
            };

            // add to the total blob gas used if the transaction successfully executed
            if let Some(tx_blob_gas) = tx.blob_gas_used() {
                sum_blob_gas_used += tx_blob_gas;

                // if we've reached the max data gas per block, we can skip blob txs entirely
                if sum_blob_gas_used == blob_params.max_blob_gas_per_block() {
                    best_txs.skip_blobs();
                }
            }

            // add gas used by the transaction to cumulative gas used, before creating the receipt
            cumulative_gas_used += gas_used;
        }

        let BlockBuilderOutcome { execution_result, block, .. } =
            builder.finish(&state_provider).map_err(Self::Error::from_eth_err)?;

        let _execution_outcome = ExecutionOutcome::new(
            db.take_bundle(),
            vec![execution_result.receipts],
            block.number(),
            vec![execution_result.requests],
        );
        let receipts = self
            .provider()
            .receipts_by_block(block.hash().into())?
            .ok_or(EthApiError::ReceiptsNotFound(block.hash().into()))?;

        Ok(Some((Arc::new(block), Arc::new(receipts))))
    }
}

pub(super) async fn track_flashblocks(url: Url, state: SharedFlashblockState) -> eyre::Result<()> {
    let pubsub = WsConnect::new(url).into_service().await?;
    let params = vec![Value::String("pending".into()), Value::Bool(true)];
    let req: SerializedRequest =
        Request::new(Cow::Borrowed("eth_getBlockByNumber"), Id::Number(1), params)
            .try_into()
            .unwrap();

    loop {
        let resp = pubsub.send(req.clone()).await?;
        let payload = match resp.payload {
            alloy_json_rpc::ResponsePayload::Success(p) => p,
            alloy_json_rpc::ResponsePayload::Failure(_error_payload) => todo!(),
        };
        let value: serde_json::Value = serde_json::from_str(payload.get())?;
        let new_fb: FlashBlock = serde_json::from_value(value)?;

        {
            let mut s = state.write().await;

            let is_new_block = new_fb.metadata.block_number > s.metadata.block_number;
            if is_new_block {
                *s = new_fb;
            } else {
                let existing_hashes: std::collections::HashSet<_> =
                    s.diff.transactions.iter().cloned().collect();

                s.diff.transactions.extend(
                    new_fb.diff.transactions.into_iter().filter(|tx| !existing_hashes.contains(tx)),
                );
            }
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
