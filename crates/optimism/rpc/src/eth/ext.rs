use super::{OpEthApiInner, OpNodeCore};
use crate::{error::TxConditionalErr, OpEthApiError, SequencerClient};
use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_eth::erc4337::TransactionConditional;
use jsonrpsee_core::RpcResult;
use reth_provider::{BlockReaderIdExt, StateProviderFactory};
use reth_rpc_eth_api::L2EthApiExtServer;
use reth_rpc_eth_types::utils::recover_raw_transaction;
use reth_transaction_pool::{PoolTransaction, TransactionOrigin, TransactionPool};
use std::sync::Arc;

/// Maximum execution const for conditional transactions.
const MAX_CONDITIONAL_EXECUTION_COST: u64 = 5000;

/// OP-Reth `Eth` API extensions implementation.
///
/// Separate from [`super::OpEthApi`] to allow to enable it conditionally,
#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct OpEthApiExt<N: OpNodeCore> {
    /// Gateway to node's core components.
    inner: Arc<OpEthApiInner<N>>,
}

impl<N> OpEthApiExt<N>
where
    N: OpNodeCore<Provider: BlockReaderIdExt + Clone + 'static>,
{
    /// Returns the configured sequencer client, if any.
    pub(crate) fn sequencer_client(&self) -> Option<&SequencerClient> {
        self.inner.sequencer_client()
    }

    #[inline]
    fn pool(&self) -> &N::Pool {
        self.inner.eth_api.pool()
    }

    #[inline]
    fn provider(&self) -> &N::Provider {
        self.inner.eth_api.provider()
    }
}

#[async_trait::async_trait]
impl<N> L2EthApiExtServer for OpEthApiExt<N>
where
    N: OpNodeCore + 'static,
    N::Provider: BlockReaderIdExt + StateProviderFactory,
    N::Pool: TransactionPool,
{
    async fn send_raw_transaction_conditional(
        &self,
        bytes: Bytes,
        condition: TransactionConditional,
    ) -> RpcResult<B256> {
        // calculate and validate cost
        let cost = condition.cost();
        if cost > MAX_CONDITIONAL_EXECUTION_COST {
            return Err(TxConditionalErr::ConditionalCostExceeded.into());
        }

        let recovered_tx = recover_raw_transaction(&bytes).map_err(|_| {
            OpEthApiError::Eth(reth_rpc_eth_types::EthApiError::FailedToDecodeSignedTransaction)
        })?;

        let tx = <N::Pool as TransactionPool>::Transaction::from_pooled(recovered_tx);

        // get current header
        let header_not_found = || {
            OpEthApiError::Eth(reth_rpc_eth_types::EthApiError::HeaderNotFound(
                alloy_eips::BlockId::Number(BlockNumberOrTag::Latest),
            ))
        };
        let header = self
            .provider()
            .latest_header()
            .map_err(|_| header_not_found())?
            .ok_or_else(header_not_found)?;

        // check condition against header
        if !condition.has_exceeded_block_number(header.header().number()) ||
            !condition.has_exceeded_timestamp(header.header().timestamp())
        {
            return Err(TxConditionalErr::InvalidCondition.into());
        }

        // TODO: check condition against state

        if let Some(sequencer) = self.sequencer_client() {
            // If we have a sequencer client, forward the transaction
            let _ = sequencer
                .forward_raw_transaction_conditional(bytes.as_ref(), condition)
                .await
                .map_err(OpEthApiError::Sequencer)?;
            Ok(*tx.hash())
        } else {
            // otherwise, add to pool
            // TODO: include conditional
            let hash = self.pool().add_transaction(TransactionOrigin::External, tx).await.map_err(
                |e| OpEthApiError::Eth(reth_rpc_eth_types::EthApiError::PoolError(e.into())),
            )?;
            Ok(hash)
        }
    }
}
