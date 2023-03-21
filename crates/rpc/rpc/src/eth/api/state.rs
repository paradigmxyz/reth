//! Contains RPC handler implementations specific to state.

use crate::{
    eth::error::{EthApiError, EthResult},
    EthApi,
};
use reth_primitives::{Address, BlockId, BlockNumberOrTag, Bytes, H256, KECCAK_EMPTY, U256};
use reth_provider::{
    AccountProvider, BlockProvider, EvmEnvProvider, StateProvider, StateProviderFactory,
};
use reth_rpc_types::{EIP1186AccountProofResponse, StorageProof};

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
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

    pub(crate) fn get_transaction_count(
        &self,
        address: Address,
        block_id: Option<BlockId>,
    ) -> EthResult<U256> {
        let state = self.state_at_block_id_or_latest(block_id)?;
        let nonce = U256::from(state.account_nonce(address)?.unwrap_or_default());
        Ok(nonce)
    }

    pub(crate) fn storage_at(
        &self,
        address: Address,
        index: U256,
        block_id: Option<BlockId>,
    ) -> EthResult<H256> {
        let state = self.state_at_block_id_or_latest(block_id)?;
        let storage_key = H256(index.to_be_bytes());
        let value = state.storage(address, storage_key)?.unwrap_or_default();
        Ok(H256(value.to_be_bytes()))
    }

    pub(crate) fn get_proof(
        &self,
        address: Address,
        keys: Vec<H256>,
        block_id: Option<BlockId>,
    ) -> EthResult<EIP1186AccountProofResponse> {
        let block_id = block_id.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));

        // TODO: remove when HistoricalStateProviderRef::proof is implemented
        if !block_id.is_latest() {
            return Err(EthApiError::InvalidBlockRange)
        }

        let state = self.state_at_block_id(block_id)?;

        let (account_proof, storage_hash, stg_proofs) = state.proof(address, &keys)?;

        let storage_proof = keys
            .into_iter()
            .zip(stg_proofs)
            .map(|(key, proof)| {
                state.storage(address, key).map(|op| StorageProof {
                    key: U256::from_be_bytes(*key.as_fixed_bytes()),
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

        if let Some(account) = state.basic_account(address)? {
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
    use crate::eth::cache::EthStateCache;
    use reth_primitives::{StorageKey, StorageValue};
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider, NoopProvider};
    use reth_transaction_pool::test_utils::testing_pool;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_storage() {
        // === Noop ===
        let pool = testing_pool();

        let eth_api = EthApi::new(
            NoopProvider::default(),
            pool.clone(),
            (),
            EthStateCache::spawn(NoopProvider::default(), Default::default()),
        );
        let address = Address::random();
        let storage = eth_api.storage_at(address, U256::ZERO, None).unwrap();
        assert_eq!(storage, U256::ZERO.into());

        // === Mock ===
        let mock_provider = MockEthProvider::default();
        let storage_value = StorageValue::from(1337);
        let storage_key = StorageKey::random();
        let storage = HashMap::from([(storage_key, storage_value)]);
        let account = ExtendedAccount::new(0, U256::ZERO).extend_storage(storage);
        mock_provider.add_account(address, account);

        let eth_api = EthApi::new(
            mock_provider.clone(),
            pool.clone(),
            (),
            EthStateCache::spawn(mock_provider, Default::default()),
        );

        let storage = eth_api.storage_at(address, storage_key.into(), None).unwrap();
        assert_eq!(storage, storage_value.into());
    }
}
