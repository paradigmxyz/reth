//! Contains RPC handler implementations specific to state.

use crate::{
    eth::error::{EthApiError, EthResult},
    EthApi,
};
use reth_primitives::{
    serde_helper::JsonStorageKey, Address, BlockId, BlockNumberOrTag, Bytes, H256, KECCAK_EMPTY,
    U256,
};
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
        index: JsonStorageKey,
        block_id: Option<BlockId>,
    ) -> EthResult<H256> {
        let state = self.state_at_block_id_or_latest(block_id)?;
        let value = state.storage(address, index.0)?.unwrap_or_default();
        Ok(H256(value.to_be_bytes()))
    }

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
