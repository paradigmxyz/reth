use crate::{
    traits::ReceiptProvider, AccountProvider, BlockHashProvider, BlockIdProvider, BlockProvider,
    EvmEnvProvider, HeaderProvider, StateProvider, StateProviderBox, StateProviderFactory,
    TransactionsProvider,
};
use reth_interfaces::Result;
use reth_primitives::{
    Account, Address, Block, BlockHash, BlockId, BlockNumber, Bytecode, Bytes, ChainInfo, Header,
    Receipt, StorageKey, StorageValue, TransactionMeta, TransactionSigned, TxHash, TxNumber, H256,
    KECCAK_EMPTY, U256,
};
use reth_revm_primitives::primitives::{BlockEnv, CfgEnv};
use std::ops::RangeBounds;

/// Supports various api interfaces for testing purposes.
#[derive(Debug, Clone, Default, Copy)]
#[non_exhaustive]
pub struct NoopProvider;

/// Noop implementation for testing purposes
impl BlockHashProvider for NoopProvider {
    fn block_hash(&self, _number: u64) -> Result<Option<H256>> {
        Ok(None)
    }

    fn canonical_hashes_range(&self, _start: BlockNumber, _end: BlockNumber) -> Result<Vec<H256>> {
        Ok(vec![])
    }
}

impl BlockIdProvider for NoopProvider {
    fn chain_info(&self) -> Result<ChainInfo> {
        Ok(ChainInfo::default())
    }

    fn block_number(&self, _hash: H256) -> Result<Option<BlockNumber>> {
        Ok(None)
    }
}

impl BlockProvider for NoopProvider {
    fn block(&self, _id: BlockId) -> Result<Option<Block>> {
        Ok(None)
    }

    fn ommers(&self, _id: BlockId) -> Result<Option<Vec<Header>>> {
        Ok(None)
    }
}

impl TransactionsProvider for NoopProvider {
    fn transaction_id(&self, _tx_hash: TxHash) -> Result<Option<TxNumber>> {
        Ok(None)
    }

    fn transaction_by_id(&self, _id: TxNumber) -> Result<Option<TransactionSigned>> {
        Ok(None)
    }

    fn transaction_by_hash(&self, _hash: TxHash) -> Result<Option<TransactionSigned>> {
        Ok(None)
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: TxHash,
    ) -> Result<Option<(TransactionSigned, TransactionMeta)>> {
        Ok(None)
    }

    fn transaction_block(&self, _id: TxNumber) -> Result<Option<BlockNumber>> {
        todo!()
    }

    fn transactions_by_block(&self, _block_id: BlockId) -> Result<Option<Vec<TransactionSigned>>> {
        Ok(None)
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<Vec<TransactionSigned>>> {
        Ok(Vec::default())
    }
}

impl ReceiptProvider for NoopProvider {
    fn receipt(&self, _id: TxNumber) -> Result<Option<Receipt>> {
        Ok(None)
    }

    fn receipt_by_hash(&self, _hash: TxHash) -> Result<Option<Receipt>> {
        Ok(None)
    }

    fn receipts_by_block(&self, _block: BlockId) -> Result<Option<Vec<Receipt>>> {
        Ok(None)
    }
}

impl HeaderProvider for NoopProvider {
    fn header(&self, _block_hash: &BlockHash) -> Result<Option<Header>> {
        Ok(None)
    }

    fn header_by_number(&self, _num: u64) -> Result<Option<Header>> {
        Ok(None)
    }

    fn header_td(&self, _hash: &BlockHash) -> Result<Option<U256>> {
        Ok(None)
    }

    fn header_td_by_number(&self, _number: BlockNumber) -> Result<Option<U256>> {
        Ok(None)
    }

    fn headers_range(&self, _range: impl RangeBounds<BlockNumber>) -> Result<Vec<Header>> {
        Ok(vec![])
    }
}

impl AccountProvider for NoopProvider {
    fn basic_account(&self, _address: Address) -> Result<Option<Account>> {
        Ok(None)
    }
}

impl StateProvider for NoopProvider {
    fn storage(&self, _account: Address, _storage_key: StorageKey) -> Result<Option<StorageValue>> {
        Ok(None)
    }

    fn bytecode_by_hash(&self, _code_hash: H256) -> Result<Option<Bytecode>> {
        Ok(None)
    }

    fn proof(
        &self,
        _address: Address,
        _keys: &[H256],
    ) -> Result<(Vec<Bytes>, H256, Vec<Vec<Bytes>>)> {
        Ok((vec![], KECCAK_EMPTY, vec![]))
    }
}

impl EvmEnvProvider for NoopProvider {
    fn fill_env_at(
        &self,
        _cfg: &mut CfgEnv,
        _block_env: &mut BlockEnv,
        _at: BlockId,
    ) -> Result<()> {
        Ok(())
    }

    fn fill_env_with_header(
        &self,
        _cfg: &mut CfgEnv,
        _block_env: &mut BlockEnv,
        _header: &Header,
    ) -> Result<()> {
        Ok(())
    }

    fn fill_block_env_at(&self, _block_env: &mut BlockEnv, _at: BlockId) -> Result<()> {
        Ok(())
    }

    fn fill_block_env_with_header(
        &self,
        _block_env: &mut BlockEnv,
        _header: &Header,
    ) -> Result<()> {
        Ok(())
    }

    fn fill_cfg_env_at(&self, _cfg: &mut CfgEnv, _at: BlockId) -> Result<()> {
        Ok(())
    }

    fn fill_cfg_env_with_header(&self, _cfg: &mut CfgEnv, _header: &Header) -> Result<()> {
        Ok(())
    }
}

impl StateProviderFactory for NoopProvider {
    fn latest(&self) -> Result<StateProviderBox<'_>> {
        Ok(Box::new(*self))
    }

    fn history_by_block_number(&self, _block: BlockNumber) -> Result<StateProviderBox<'_>> {
        Ok(Box::new(*self))
    }

    fn history_by_block_hash(&self, _block: BlockHash) -> Result<StateProviderBox<'_>> {
        Ok(Box::new(*self))
    }

    fn pending(&self) -> Result<StateProviderBox<'_>> {
        Ok(Box::new(*self))
    }

    fn pending_with_provider<'a>(
        &'a self,
        _post_state_data: Box<dyn crate::PostStateDataProvider + 'a>,
    ) -> Result<StateProviderBox<'a>> {
        Ok(Box::new(*self))
    }
}
