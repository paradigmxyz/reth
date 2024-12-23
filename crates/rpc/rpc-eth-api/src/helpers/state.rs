//! Loads a pending block from database. Helper trait for `eth_` block, transaction, call and trace
//! RPC methods.
use super::{EthApiSpec, LoadPendingBlock, SpawnBlocking};
use crate::{EthApiTypes, FromEthApiError, RpcNodeCore, RpcNodeCoreExt};
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_eips::BlockId;
use alloy_primitives::{Address, Bytes, B256, U256};
use alloy_rpc_types_eth::{Account, EIP1186AccountProofResponse};
use alloy_serde::JsonStorageKey;
use futures::Future;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_errors::RethError;
use reth_evm::{env::EvmEnv, ConfigureEvmEnv};
use reth_provider::{
    BlockIdReader, BlockNumReader, ChainSpecProvider, StateProvider, StateProviderBox,
    StateProviderFactory,
};
use reth_rpc_eth_types::{EthApiError, PendingBlockEnv, RpcInvalidTransactionError};
use reth_transaction_pool::TransactionPool;

/// Helper methods for `eth_` methods relating to state (accounts).
pub trait EthState: LoadState + SpawnBlocking {
    /// Returns the maximum number of blocks into the past for generating state proofs.
    fn max_proof_window(&self) -> u64;

    /// Returns the number of transactions sent from an address at the given block identifier.
    ///
    /// If this is [`BlockNumberOrTag::Pending`](alloy_eips::BlockNumberOrTag) then this will
    /// look up the highest transaction in pool and return the next nonce (highest + 1).
    fn transaction_count(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> impl Future<Output = Result<U256, Self::Error>> + Send {
        LoadState::transaction_count(self, address, block_id)
    }

    /// Returns code of given account, at given blocknumber.
    fn get_code(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> impl Future<Output = Result<Bytes, Self::Error>> + Send {
        LoadState::get_code(self, address, block_id)
    }

    /// Returns balance of given account, at given blocknumber.
    fn balance(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> impl Future<Output = Result<U256, Self::Error>> + Send {
        self.spawn_blocking_io(move |this| {
            Ok(this
                .state_at_block_id_or_latest(block_id)?
                .account_balance(address)
                .map_err(Self::Error::from_eth_err)?
                .unwrap_or_default())
        })
    }

    /// Returns values stored of given account, at given blocknumber.
    fn storage_at(
        &self,
        address: Address,
        index: JsonStorageKey,
        block_id: Option<BlockId>,
    ) -> impl Future<Output = Result<B256, Self::Error>> + Send {
        self.spawn_blocking_io(move |this| {
            Ok(B256::new(
                this.state_at_block_id_or_latest(block_id)?
                    .storage(address, index.as_b256())
                    .map_err(Self::Error::from_eth_err)?
                    .unwrap_or_default()
                    .to_be_bytes(),
            ))
        })
    }

    /// Returns values stored of given account, with Merkle-proof, at given blocknumber.
    fn get_proof(
        &self,
        address: Address,
        keys: Vec<JsonStorageKey>,
        block_id: Option<BlockId>,
    ) -> Result<
        impl Future<Output = Result<EIP1186AccountProofResponse, Self::Error>> + Send,
        Self::Error,
    >
    where
        Self: EthApiSpec,
    {
        Ok(async move {
            let _permit = self
                .acquire_owned()
                .await
                .map_err(RethError::other)
                .map_err(EthApiError::Internal)?;

            let chain_info = self.chain_info().map_err(Self::Error::from_eth_err)?;
            let block_id = block_id.unwrap_or_default();

            // Check whether the distance to the block exceeds the maximum configured window.
            let block_number = self
                .provider()
                .block_number_for_id(block_id)
                .map_err(Self::Error::from_eth_err)?
                .ok_or(EthApiError::HeaderNotFound(block_id))?;
            let max_window = self.max_proof_window();
            if chain_info.best_number.saturating_sub(block_number) > max_window {
                return Err(EthApiError::ExceedsMaxProofWindow.into())
            }

            self.spawn_blocking_io(move |this| {
                let state = this.state_at_block_id(block_id)?;
                let storage_keys = keys.iter().map(|key| key.as_b256()).collect::<Vec<_>>();
                let proof = state
                    .proof(Default::default(), address, &storage_keys)
                    .map_err(Self::Error::from_eth_err)?;
                Ok(proof.into_eip1186_response(keys))
            })
            .await
        })
    }

    /// Returns the account at the given address for the provided block identifier.
    fn get_account(
        &self,
        address: Address,
        block_id: BlockId,
    ) -> impl Future<Output = Result<Option<Account>, Self::Error>> + Send {
        self.spawn_blocking_io(move |this| {
            let state = this.state_at_block_id(block_id)?;
            let account = state.basic_account(address).map_err(Self::Error::from_eth_err)?;
            let Some(account) = account else { return Ok(None) };

            // Check whether the distance to the block exceeds the maximum configured proof window.
            let chain_info = this.provider().chain_info().map_err(Self::Error::from_eth_err)?;
            let block_number = this
                .provider()
                .block_number_for_id(block_id)
                .map_err(Self::Error::from_eth_err)?
                .ok_or(EthApiError::HeaderNotFound(block_id))?;
            let max_window = this.max_proof_window();
            if chain_info.best_number.saturating_sub(block_number) > max_window {
                return Err(EthApiError::ExceedsMaxProofWindow.into())
            }

            let balance = account.balance;
            let nonce = account.nonce;
            let code_hash = account.bytecode_hash.unwrap_or(KECCAK_EMPTY);

            // Provide a default `HashedStorage` value in order to
            // get the storage root hash of the current state.
            let storage_root = state
                .storage_root(address, Default::default())
                .map_err(Self::Error::from_eth_err)?;

            Ok(Some(Account { balance, nonce, code_hash, storage_root }))
        })
    }
}

/// Loads state from database.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` state RPC methods.
pub trait LoadState:
    EthApiTypes
    + RpcNodeCoreExt<
        Provider: StateProviderFactory
                      + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>,
        Pool: TransactionPool,
    >
{
    /// Returns the state at the given block number
    fn state_at_hash(&self, block_hash: B256) -> Result<StateProviderBox, Self::Error> {
        self.provider().history_by_block_hash(block_hash).map_err(Self::Error::from_eth_err)
    }

    /// Returns the state at the given [`BlockId`] enum.
    ///
    /// Note: if not [`BlockNumberOrTag::Pending`](alloy_eips::BlockNumberOrTag) then this
    /// will only return canonical state. See also <https://github.com/paradigmxyz/reth/issues/4515>
    fn state_at_block_id(&self, at: BlockId) -> Result<StateProviderBox, Self::Error> {
        self.provider().state_by_block_id(at).map_err(Self::Error::from_eth_err)
    }

    /// Returns the _latest_ state
    fn latest_state(&self) -> Result<StateProviderBox, Self::Error> {
        self.provider().latest().map_err(Self::Error::from_eth_err)
    }

    /// Returns the state at the given [`BlockId`] enum or the latest.
    ///
    /// Convenience function to interprets `None` as `BlockId::Number(BlockNumberOrTag::Latest)`
    fn state_at_block_id_or_latest(
        &self,
        block_id: Option<BlockId>,
    ) -> Result<StateProviderBox, Self::Error> {
        if let Some(block_id) = block_id {
            self.state_at_block_id(block_id)
        } else {
            Ok(self.latest_state()?)
        }
    }

    /// Returns the revm evm env for the requested [`BlockId`]
    ///
    /// If the [`BlockId`] this will return the [`BlockId`] of the block the env was configured
    /// for.
    /// If the [`BlockId`] is pending, this will return the "Pending" tag, otherwise this returns
    /// the hash of the exact block.
    fn evm_env_at(
        &self,
        at: BlockId,
    ) -> impl Future<Output = Result<(EvmEnv, BlockId), Self::Error>> + Send
    where
        Self: LoadPendingBlock + SpawnBlocking,
    {
        async move {
            if at.is_pending() {
                let PendingBlockEnv { cfg, block_env, origin } =
                    self.pending_block_env_and_cfg()?;
                Ok(((cfg, block_env).into(), origin.state_block_id()))
            } else {
                // Use cached values if there is no pending block
                let block_hash = RpcNodeCore::provider(self)
                    .block_hash_for_id(at)
                    .map_err(Self::Error::from_eth_err)?
                    .ok_or(EthApiError::HeaderNotFound(at))?;

                let header =
                    self.cache().get_header(block_hash).await.map_err(Self::Error::from_eth_err)?;
                let evm_env = self.evm_config().cfg_and_block_env(&header);

                Ok((evm_env, block_hash.into()))
            }
        }
    }

    /// Returns the next available nonce without gaps for the given address
    /// Next available nonce is either the on chain nonce of the account or the highest consecutive
    /// nonce in the pool + 1
    fn next_available_nonce(
        &self,
        address: Address,
    ) -> impl Future<Output = Result<u64, Self::Error>> + Send
    where
        Self: SpawnBlocking,
    {
        self.spawn_blocking_io(move |this| {
            // first fetch the on chain nonce of the account
            let on_chain_account_nonce = this
                .latest_state()?
                .account_nonce(address)
                .map_err(Self::Error::from_eth_err)?
                .unwrap_or_default();

            let mut next_nonce = on_chain_account_nonce;
            // Retrieve the highest consecutive transaction for the sender from the transaction pool
            if let Some(highest_tx) = this
                .pool()
                .get_highest_consecutive_transaction_by_sender(address, on_chain_account_nonce)
            {
                // Return the nonce of the highest consecutive transaction + 1
                next_nonce = highest_tx.nonce().checked_add(1).ok_or_else(|| {
                    Self::Error::from(EthApiError::InvalidTransaction(
                        RpcInvalidTransactionError::NonceMaxValue,
                    ))
                })?;
            }

            Ok(next_nonce)
        })
    }

    /// Returns the number of transactions sent from an address at the given block identifier.
    ///
    /// If this is [`BlockNumberOrTag::Pending`](alloy_eips::BlockNumberOrTag) then this will
    /// look up the highest transaction in pool and return the next nonce (highest + 1).
    fn transaction_count(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> impl Future<Output = Result<U256, Self::Error>> + Send
    where
        Self: SpawnBlocking,
    {
        self.spawn_blocking_io(move |this| {
            // first fetch the on chain nonce of the account
            let on_chain_account_nonce = this
                .state_at_block_id_or_latest(block_id)?
                .account_nonce(address)
                .map_err(Self::Error::from_eth_err)?
                .unwrap_or_default();

            if block_id == Some(BlockId::pending()) {
                // for pending tag we need to find the highest nonce in the pool
                if let Some(highest_pool_tx) =
                    this.pool().get_highest_transaction_by_sender(address)
                {
                    {
                        // and the corresponding txcount is nonce + 1 of the highest tx in the pool
                        // (on chain nonce is increased after tx)
                        let next_tx_nonce =
                            highest_pool_tx.nonce().checked_add(1).ok_or_else(|| {
                                Self::Error::from(EthApiError::InvalidTransaction(
                                    RpcInvalidTransactionError::NonceMaxValue,
                                ))
                            })?;

                        // guard against drifts in the pool
                        let next_tx_nonce = on_chain_account_nonce.max(next_tx_nonce);

                        let tx_count = on_chain_account_nonce.max(next_tx_nonce);
                        return Ok(U256::from(tx_count));
                    }
                }
            }
            Ok(U256::from(on_chain_account_nonce))
        })
    }

    /// Returns code of given account, at the given identifier.
    fn get_code(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> impl Future<Output = Result<Bytes, Self::Error>> + Send
    where
        Self: SpawnBlocking,
    {
        self.spawn_blocking_io(move |this| {
            Ok(this
                .state_at_block_id_or_latest(block_id)?
                .account_code(address)
                .map_err(Self::Error::from_eth_err)?
                .unwrap_or_default()
                .original_bytes())
        })
    }
}
