//! Eth API extension.

use super::transaction::ensure_transaction_input_supported;
use crate::{error::TxConditionalErr, OpEthApiError, SequencerClient};
use alloy_consensus::{BlockHeader, Transaction};
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{Bytes, StorageKey, B256, U256};
use alloy_rpc_types_eth::erc4337::{AccountStorage, TransactionConditional};
use jsonrpsee_core::RpcResult;
use reth_optimism_txpool::conditional::MaybeConditionalTransaction;
use reth_rpc_eth_api::L2EthApiExtServer;
use reth_rpc_eth_types::utils::recover_raw_transaction;
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use reth_transaction_pool::{
    AddedTransactionOutcome, PoolTransaction, TransactionOrigin, TransactionPool,
};
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Maximum execution const for conditional transactions.
const MAX_CONDITIONAL_EXECUTION_COST: u64 = 5000;

const MAX_CONCURRENT_CONDITIONAL_VALIDATIONS: usize = 3;

/// OP-Reth `Eth` API extensions implementation.
///
/// Separate from [`super::OpEthApi`] to allow to enable it conditionally,
/// Mantle methods are in [`super::mantle_ext::MantleEthApiExt`].
#[derive(Clone, Debug)]
pub struct OpEthExtApi<Pool, Provider> {
    /// Sequencer client, configured to forward submitted transactions to sequencer of given OP
    /// network.
    sequencer_client: Option<SequencerClient>,
    inner: Arc<OpEthExtApiInner<Pool, Provider>>,
}

impl<Pool, Provider> OpEthExtApi<Pool, Provider>
where
    Provider: BlockReaderIdExt + StateProviderFactory + Clone + 'static,
{
    /// Creates a new [`OpEthExtApi`].
    pub fn new(sequencer_client: Option<SequencerClient>, pool: Pool, provider: Provider) -> Self {
        let inner = Arc::new(OpEthExtApiInner::new(pool, provider));
        Self { sequencer_client, inner }
    }

    /// Returns the configured sequencer client, if any.
    const fn sequencer_client(&self) -> Option<&SequencerClient> {
        self.sequencer_client.as_ref()
    }

    #[inline]
    fn pool(&self) -> &Pool {
        self.inner.pool()
    }

    #[inline]
    fn provider(&self) -> &Provider {
        self.inner.provider()
    }

    /// Validates the conditional's `known accounts` settings against the current state.
    async fn validate_known_accounts(
        &self,
        condition: &TransactionConditional,
    ) -> Result<(), TxConditionalErr> {
        if condition.known_accounts.is_empty() {
            return Ok(());
        }

        let _permit =
            self.inner.validation_semaphore.acquire().await.map_err(TxConditionalErr::internal)?;

        let state = self
            .provider()
            .state_by_block_number_or_tag(BlockNumberOrTag::Latest)
            .map_err(TxConditionalErr::internal)?;

        for (address, storage) in &condition.known_accounts {
            match storage {
                AccountStorage::Slots(slots) => {
                    for (slot, expected_value) in slots {
                        let current = state
                            .storage(*address, StorageKey::from(*slot))
                            .map_err(TxConditionalErr::internal)?
                            .unwrap_or_default();

                        if current != U256::from_be_bytes(**expected_value) {
                            return Err(TxConditionalErr::StorageValueMismatch);
                        }
                    }
                }
                AccountStorage::RootHash(expected_root) => {
                    let actual_root = state
                        .storage_root(*address, Default::default())
                        .map_err(TxConditionalErr::internal)?;

                    if *expected_root != actual_root {
                        return Err(TxConditionalErr::StorageRootMismatch);
                    }
                }
            }
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl<Pool, Provider> L2EthApiExtServer for OpEthExtApi<Pool, Provider>
where
    Provider: BlockReaderIdExt + StateProviderFactory + Clone + Send + Sync + 'static,
    Pool: TransactionPool<Transaction: MaybeConditionalTransaction> + Send + Sync + 'static,
{
    async fn send_raw_transaction_conditional(
        &self,
        bytes: Bytes,
        condition: TransactionConditional,
    ) -> RpcResult<B256> {
        let recovered_tx = recover_raw_transaction(&bytes).map_err(|_| {
            OpEthApiError::Eth(reth_rpc_eth_types::EthApiError::FailedToDecodeSignedTransaction)
        })?;

        let mut tx = <Pool as TransactionPool>::Transaction::from_pooled(recovered_tx);
        ensure_transaction_input_supported(tx.input()).map_err(OpEthApiError::Eth)?;

        // calculate and validate cost
        let cost = condition.cost();
        if cost > MAX_CONDITIONAL_EXECUTION_COST {
            return Err(TxConditionalErr::ConditionalCostExceeded.into());
        }

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

        // Ensure that the condition can still be met by checking the max bounds
        if condition.has_exceeded_block_number(header.header().number()) ||
            condition.has_exceeded_timestamp(header.header().timestamp())
        {
            return Err(TxConditionalErr::InvalidCondition.into());
        }

        // Validate Account
        self.validate_known_accounts(&condition).await?;

        if let Some(sequencer) = self.sequencer_client() {
            // If we have a sequencer client, forward the transaction
            let _ = sequencer
                .forward_raw_transaction_conditional(bytes.as_ref(), condition)
                .await
                .map_err(OpEthApiError::Sequencer)?;
            Ok(*tx.hash())
        } else {
            // otherwise, add to pool with the appended conditional
            tx.set_conditional(condition);
            let AddedTransactionOutcome { hash, .. } =
                self.pool().add_transaction(TransactionOrigin::Private, tx).await.map_err(|e| {
                    OpEthApiError::Eth(reth_rpc_eth_types::EthApiError::PoolError(e.into()))
                })?;

            Ok(hash)
        }
    }
}

#[derive(Debug)]
struct OpEthExtApiInner<Pool, Provider> {
    /// The transaction pool of the node.
    pool: Pool,
    /// The provider type used to interact with the node.
    provider: Provider,
    /// The semaphore used to limit the number of concurrent conditional validations.
    validation_semaphore: Semaphore,
}

impl<Pool, Provider> OpEthExtApiInner<Pool, Provider> {
    fn new(pool: Pool, provider: Provider) -> Self {
        Self {
            pool,
            provider,
            validation_semaphore: Semaphore::new(MAX_CONCURRENT_CONDITIONAL_VALIDATIONS),
        }
    }

    #[inline]
    const fn pool(&self) -> &Pool {
        &self.pool
    }

    #[inline]
    const fn provider(&self) -> &Provider {
        &self.provider
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{SignableTransaction, TxEip1559};
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{Address, Signature, TxKind};
    use alloy_rpc_types_eth::erc4337::AccountStorage;
    use reth_mantle_forks::MANTLE_META_TX_PREFIX;
    use reth_optimism_txpool::OpPooledTransaction;
    use reth_storage_api::noop::NoopProvider;
    use reth_transaction_pool::noop::NoopTransactionPool;

    fn raw_eip1559_with_input(input: Bytes) -> Bytes {
        TxEip1559 {
            chain_id: 5000,
            nonce: 0,
            gas_limit: 100_000,
            max_fee_per_gas: 1,
            max_priority_fee_per_gas: 1,
            to: TxKind::Call(Address::ZERO),
            input,
            ..Default::default()
        }
        .into_signed(Signature::test_signature())
        .encoded_2718()
        .into()
    }

    fn mantle_meta_tx_raw() -> Bytes {
        let mut input = MANTLE_META_TX_PREFIX.to_vec();
        input.push(0xF8);
        raw_eip1559_with_input(input.into())
    }

    fn over_budget_condition() -> TransactionConditional {
        let mut condition = TransactionConditional::default();
        for i in 0..=MAX_CONDITIONAL_EXECUTION_COST {
            let mut address = [0u8; 20];
            address[12..].copy_from_slice(&i.to_be_bytes());
            condition
                .known_accounts
                .insert(Address::from(address), AccountStorage::RootHash(Default::default()));
        }
        condition
    }

    #[tokio::test]
    async fn send_raw_transaction_conditional_rejects_mantle_meta_tx_before_condition_validation() {
        let api: OpEthExtApi<NoopTransactionPool<OpPooledTransaction>, NoopProvider> =
            OpEthExtApi::new(
                None,
                NoopTransactionPool::<OpPooledTransaction>::new(),
                NoopProvider::default(),
            );

        let err = api
            .send_raw_transaction_conditional(mantle_meta_tx_raw(), over_budget_condition())
            .await
            .unwrap_err();

        assert_eq!(err.code(), -32000);
        assert_eq!(err.message(), "meta tx is disabled");
    }
}
