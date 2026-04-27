//! Helpers for preparing state with block access lists.

use super::{LoadState, SpawnBlocking};
use crate::FromEvmError;
use alloy_eip7928::bal::Bal as AlloyBal;
use alloy_eips::BlockId;
use alloy_primitives::B256;
use futures::Future;
use reth_evm::{ConfigureEvm, Evm, EvmEnvFor};
use reth_primitives_traits::Recovered;
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_rpc_eth_types::{cache::db::StateProviderTraitObjWrapper, StateCacheDb};
use reth_storage_api::{BalProvider, ProviderTx};
use revm::state::bal::Bal;
use std::sync::Arc;
use tracing::debug;

/// Prepares RPC state with block access list data when it is available.
pub trait BlockAccessListState: LoadState + SpawnBlocking {
    /// Executes the closure with state for `at` and, when available, the BAL for `block_hash`.
    fn spawn_with_state_at_block_and_bal<F, R>(
        &self,
        at: impl Into<BlockId>,
        block_hash: B256,
        f: F,
    ) -> impl Future<Output = Result<R, Self::Error>> + Send
    where
        F: FnOnce(Self, StateCacheDb) -> Result<R, Self::Error> + Send + 'static,
        R: Send + 'static,
    {
        let at = at.into();
        self.spawn_blocking_io_fut(async move |this| {
            let state = this.state_at_block_id(at).await?;
            let mut db = State::builder()
                .with_database(StateProviderDatabase::new(StateProviderTraitObjWrapper(state)))
                .build();
            this.attach_block_bal(block_hash, &mut db);
            f(this, db)
        })
    }

    /// Loads the block access list into `db` when it is available.
    fn attach_block_bal(&self, block_hash: B256, db: &mut StateCacheDb) {
        if let Some(bal) = self.load_revm_block_bal(block_hash) {
            db.set_bal(Some(bal));
        }
    }

    /// Fetches and decodes the block access list into the revm representation.
    fn load_revm_block_bal(&self, block_hash: B256) -> Option<Arc<Bal>> {
        let raw_bal = match self.provider().bal_store().get_by_hashes(&[block_hash]) {
            Ok(bals) => bals.into_iter().next().flatten()?,
            Err(err) => {
                debug!(
                    target: "reth::rpc",
                    ?block_hash,
                    %err,
                    "Failed to fetch block access list"
                );
                return None
            }
        };

        let alloy_bal = match alloy_rlp::decode_exact::<AlloyBal>(raw_bal.as_ref()) {
            Ok(bal) => bal,
            Err(err) => {
                debug!(
                    target: "reth::rpc",
                    ?block_hash,
                    %err,
                    "Failed to decode block access list"
                );
                return None
            }
        };

        match Bal::try_from(alloy_bal.into_inner()) {
            Ok(bal) => Some(Arc::new(bal)),
            Err(err) => {
                debug!(
                    target: "reth::rpc",
                    ?block_hash,
                    %err,
                    "Failed to convert block access list"
                );
                None
            }
        }
    }

    /// Positions `db` at the state before the transaction at `target_tx_index`.
    ///
    /// If a block access list is attached, this only sets the BAL index. Otherwise it executes and
    /// commits the transactions preceding `target_tx_index`.
    fn position_state_before_transaction<'a, I>(
        &self,
        db: &mut StateCacheDb,
        evm_env: EvmEnvFor<Self::Evm>,
        transactions: I,
        target_tx_index: usize,
    ) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Recovered<&'a ProviderTx<Self::Provider>>>,
    {
        if db.bal_state.bal.is_some() {
            db.set_bal_index(target_tx_index as u64 + 1);
            return Ok(())
        }

        let mut evm = self.evm_config().evm_with_env(db, evm_env);
        for tx in transactions.into_iter().take(target_tx_index) {
            let tx_env = self.evm_config().tx_env(tx);
            evm.transact_commit(tx_env).map_err(Self::Error::from_evm_err)?;
        }
        Ok(())
    }
}

impl<T> BlockAccessListState for T where T: LoadState + SpawnBlocking {}
