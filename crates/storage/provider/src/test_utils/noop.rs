use crate::{
    AccountProvider, BlockHashProvider, BlockIdProvider, BlockProvider, EvmEnvProvider,
    HeaderProvider, StateProvider, StateProviderFactory, TransactionsProvider,
};
use reth_interfaces::Result;
use reth_primitives::{
    Account, Address, Block, BlockHash, BlockId, BlockNumber, Bytecode, ChainInfo, Header,
    StorageKey, StorageValue, TransactionSigned, TxHash, TxNumber, H256, U256,
};
use revm_primitives::{BlockEnv, CfgEnv};
use std::ops::RangeBounds;

/// Supports various api interfaces for testing purposes.
#[derive(Debug, Clone, Default, Copy)]
#[non_exhaustive]
pub struct NoopProvider;

/// Noop implementation for testing purposes
impl BlockHashProvider for NoopProvider {
    fn block_hash(&self, _number: U256) -> Result<Option<H256>> {
        Ok(None)
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
    fn transaction_by_id(&self, _id: TxNumber) -> Result<Option<TransactionSigned>> {
        Ok(None)
    }

    fn transaction_by_hash(&self, _hash: TxHash) -> Result<Option<TransactionSigned>> {
        Ok(None)
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
    type HistorySP<'a> = NoopProvider where Self: 'a;
    type LatestSP<'a> = NoopProvider where Self: 'a;

    fn latest(&self) -> Result<Self::LatestSP<'_>> {
        Ok(*self)
    }

    fn history_by_block_number(&self, _block: BlockNumber) -> Result<Self::HistorySP<'_>> {
        Ok(*self)
    }

    fn history_by_block_hash(&self, _block: BlockHash) -> Result<Self::HistorySP<'_>> {
        Ok(*self)
    }
}
