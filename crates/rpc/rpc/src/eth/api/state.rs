//! Contains RPC handler implementations specific to state.

use crate::{
    eth::error::{EthApiError, EthResult, InvalidTransactionError},
    EthApi,
};
use reth_primitives::{
    serde_helper::JsonStorageKey, Address, BlockId, BlockNumberOrTag, Bytes, H256, KECCAK_EMPTY,
    U256,
};
use reth_provider::{
    AccountProvider, BlockProviderIdExt, EvmEnvProvider, StateProvider, StateProviderFactory,
};
use reth_rpc_types::{EIP1186AccountProofResponse, StorageProof};
use reth_transaction_pool::{PoolTransaction, TransactionPool};

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Client: BlockProviderIdExt + StateProviderFactory + EvmEnvProvider + 'static,
    Pool: TransactionPool + Clone + 'static,
{
    pub(crate) fn get_code(&self, address: Address, block_id: Option<BlockId>) -> EthResult<Bytes> {
        let state = self.state_at_block_id_or_latest(block_id)?;
        let code = state.account_code(address)?.unwrap_or_default();
        Ok(code.original_bytes().into())
    }

    pub(crate) fn balance(&self, address: Address, block_id: Option<BlockId>) -> EthResult<U256> {
        let state = self.state_at_block_id_or_latest(block_id)?;
        let balance = state.account_balance(address)?.unwrap_or_default();
        Ok(balance)
    }

    /// Returns the number of transactions sent from an address at the given block identifier.
    ///
    /// If this is [BlockNumberOrTag::Pending] then this will look up the highest transaction in
    /// pool and return the next nonce (highest + 1).
    pub(crate) fn get_transaction_count(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> EthResult<U256> {
        if let Some(BlockId::Number(BlockNumberOrTag::Pending)) = block_id {
            // lookup transactions in pool
            let address_txs = self.pool().get_transactions_by_sender(address);

            if !address_txs.is_empty() {
                // get max transaction with the highest nonce
                let highest_nonce_tx = address_txs
                    .into_iter()
                    .reduce(|accum, item| {
                        if item.transaction.nonce() > accum.transaction.nonce() {
                            item
                        } else {
                            accum
                        }
                    })
                    .expect("Not empty; qed");

                let tx_count = highest_nonce_tx
                    .transaction
                    .nonce()
                    .checked_add(1)
                    .ok_or(InvalidTransactionError::NonceMaxValue)?;
                return Ok(U256::from(tx_count))
            }
        }

        let state = self.state_at_block_id_or_latest(block_id)?;
        Ok(U256::from(state.account_nonce(address)?.unwrap_or_default()))
    }

    pub(crate) fn storage_at(
        &self,
        address: Address,
        index: JsonStorageKey,
        block_id: Option<BlockId>,
    ) -> EthResult<H256> {
        let state = self.state_at_block_id_or_latest(block_id)?;
        let value = state.storage(address, index.0)?.unwrap_or_default();
        Ok(H256(value.to_be_bytes()))
    }

    #[allow(unused)]
    pub(crate) fn get_proof(
        &self,
        address: Address,
        keys: Vec<JsonStorageKey>,
        block_id: Option<BlockId>,
    ) -> EthResult<EIP1186AccountProofResponse> {
        let chain_info = self.client().chain_info()?;
        let block_id = block_id.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));

        // if we are trying to create a proof for the latest block, but have a BlockId as input
        // that is not BlockNumberOrTag::Latest, then we need to figure out whether or not the
        // BlockId corresponds to the latest block
        let is_blockid_latest = match block_id {
            BlockId::Number(BlockNumberOrTag::Number(num)) => num == chain_info.best_number,
            BlockId::Hash(hash) => hash == chain_info.best_hash.into(),
            BlockId::Number(BlockNumberOrTag::Latest) => true,
            _ => false,
        };

        // TODO: remove when HistoricalStateProviderRef::proof is implemented
        if !is_blockid_latest {
            return Err(EthApiError::InvalidBlockRange)
        }

        let state = self.state_at_block_id(block_id)?;

        let hash_keys = keys.iter().map(|key| key.0).collect::<Vec<_>>();
        let (account_proof, storage_hash, stg_proofs) = state.proof(address, &hash_keys)?;

        let storage_proof = keys
            .into_iter()
            .zip(stg_proofs)
            .map(|(key, proof)| {
                state.storage(address, key.0).map(|op| StorageProof {
                    key,
                    value: op.unwrap_or_default(),
                    proof,
                })
            })
            .collect::<Result<_, _>>()?;

        let mut proof = EIP1186AccountProofResponse {
            address,
            code_hash: KECCAK_EMPTY,
            account_proof,
            storage_hash,
            storage_proof,
            ..Default::default()
        };

        if let Some(account) = state.basic_account(proof.address)? {
            proof.balance = account.balance;
            proof.nonce = account.nonce.into();
            proof.code_hash = account.get_bytecode_hash();
        }

        Ok(proof)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::{cache::EthStateCache, gas_oracle::GasPriceOracle};
    use reth_primitives::{StorageKey, StorageValue};
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider, NoopProvider};
    use reth_transaction_pool::test_utils::testing_pool;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_storage() {
        // === Noop ===
        let pool = testing_pool();

        let cache = EthStateCache::spawn(NoopProvider::default(), Default::default());
        let eth_api = EthApi::new(
            NoopProvider::default(),
            pool.clone(),
            (),
            cache.clone(),
            GasPriceOracle::new(NoopProvider::default(), Default::default(), cache),
        );
        let address = Address::random();
        let storage = eth_api.storage_at(address, U256::ZERO.into(), None).unwrap();
        assert_eq!(storage, U256::ZERO.into());

        // === Mock ===
        let mock_provider = MockEthProvider::default();
        let storage_value = StorageValue::from(1337);
        let storage_key = StorageKey::random();
        let storage = HashMap::from([(storage_key, storage_value)]);
        let account = ExtendedAccount::new(0, U256::ZERO).extend_storage(storage);
        mock_provider.add_account(address, account);

        let cache = EthStateCache::spawn(mock_provider.clone(), Default::default());
        let eth_api = EthApi::new(
            mock_provider.clone(),
            pool,
            (),
            cache.clone(),
            GasPriceOracle::new(mock_provider, Default::default(), cache),
        );

        let storage_key: U256 = storage_key.into();
        let storage = eth_api.storage_at(address, storage_key.into(), None).unwrap();
        assert_eq!(storage, storage_value.into());
    }
}
