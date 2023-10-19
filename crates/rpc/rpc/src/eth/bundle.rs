use crate::{
    eth::{
        error::{EthApiError, EthResult},
        revm_utils::FillableTransaction,
        utils::recover_raw_transaction,
        EthTransactions,
    },
    BlockingTaskGuard,
};
use reth_primitives::U256;
use reth_revm::{database::StateProviderDatabase, env::tx_env_with_recovered};
use reth_rpc_types::{EthCallBundle, EthCallBundleResponse, EthCallBundleTransactionResult};
use revm::{
    db::{CacheDB, EmptyDB},
    precompile::Precompiles,
    primitives::{BlockEnv, CfgEnv, Env, ResultAndState, SpecId, TransactTo, TxEnv},
    Database, Inspector,
};
use revm_primitives::db::{DatabaseCommit, DatabaseRef};
use std::sync::Arc;

/// `Eth` bundle implementation.
pub struct EthBundle<Eth> {
    /// All nested fields bundled together.
    inner: Arc<EthBundleInner<Eth>>,
}

impl<Eth> EthBundle<Eth> {
    /// Create a new `EthBundle` instance.
    pub fn new(eth_api: Eth, blocking_task_guard: BlockingTaskGuard) -> Self {
        Self { inner: Arc::new(EthBundleInner { eth_api, blocking_task_guard }) }
    }
}

impl<Eth> EthBundle<Eth>
where
    Eth: EthTransactions + 'static,
{
    /// Simulates a bundle of transactions at the top of a given block number with the state of
    /// another (or the same) block. This can be used to simulate future blocks with the current
    /// state, or it can be used to simulate a past block. The sender is responsible for signing the
    /// transactions and using the correct nonce and ensuring validity
    pub async fn call_bundle(&self, bundle: EthCallBundle) -> EthResult<EthCallBundleResponse> {
        let EthCallBundle { txs, block_number, state_block_number, timestamp } = bundle;

        let transactions =
            txs.into_iter().map(recover_raw_transaction).collect::<Result<Vec<_>, _>>()?;

        let ((cfg, mut block_env, at), block) = futures::try_join!(
            self.inner.eth_api.evm_env_at(state_block_number.into()),
            self.inner.eth_api.block_by_id(state_block_number.into())
        )?;

        let block = match block {
            Some(block) => block,
            None => return Err(EthApiError::UnknownBlockNumber),
        };

        // need to adjust the timestamp for the next block
        if let Some(timestamp) = timestamp {
            block_env.timestamp = U256::from(timestamp);
        } else {
            block_env.timestamp += U256::from(12);
        }

        // use the block number of the request
        block_env.number = U256::from(block_number);

        self.inner
            .eth_api
            .spawn_with_state_at_block(at, move |state| {
                let coinbase = block_env.beneficiary;
                let basefee = block_env.basefee.map(|fee|fee.to::<u64>());
                let env = Env { cfg, block: block_env, tx: TxEnv::default() };
                let mut evm = revm::EVM::with_env(env);
                let mut db = CacheDB::new(StateProviderDatabase::new(state));

                let initial_coinbase = DatabaseRef::basic(&db, coinbase)?.map(|acc|acc.balance)).unwrap_or_default();
                let mut total_gas_used = 0u64;

                evm.database(db);
                // let mut results = Vec::with_capacity(transactions.len());

                let mut transactions = transactions.into_iter().peekable();

                while let Some(tx) = transactions.next() {
                    let tx = tx.into_ecrecovered_transaction();
                    let gas_price = tx.effective_gas_tip(basefee);
                    tx.try_fill_tx_env(&mut evm.env.tx)?;
                    let res = evm.transact()?;

                    let tx_res = EthCallBundleTransactionResult {
                        coinbase_diff: todo!(),
                        eth_sent_to_coinbase: todo!(),
                        from_address: todo!(),
                        gas_fees: todo!(),
                        gas_price: todo!(),
                        gas_used: todo!(),
                        to_address: todo!(),
                        tx_hash: todo!(),
                        value: todo!(),
                    };
                    
                    // need to apply the state changes of this call before executing the
                    // next call
                    if transactions.peek().is_some() {
                        // need to apply the state changes of this call before executing
                        // the next call
                        evm.db.as_mut().expect("is set").commit(res.state)
                    }
                }

                todo!()
            })
            .await
    }
}

/// Container type for  `EthBundle` internals
#[derive(Debug)]
struct EthBundleInner<Eth> {
    /// Access to commonly used code of the `eth` namespace
    eth_api: Eth,
    // restrict the number of concurrent tracing calls.
    blocking_task_guard: BlockingTaskGuard,
}

impl<Eth> std::fmt::Debug for EthBundle<Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthBundle").finish_non_exhaustive()
    }
}

impl<Eth> Clone for EthBundle<Eth> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}
