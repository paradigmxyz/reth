use crate::{
    AccountProvider, BlockHashProvider, BlockProvider, HeaderProvider, StateProvider,
    StateProviderFactory,
};
use reth_interfaces::Result;
use reth_primitives::{
    rpc::BlockId, Account, Address, Block, BlockHash, BlockNumber, Bytes, ChainInfo, Header,
    StorageKey, StorageValue, H256, U256,
};

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

impl BlockProvider for NoopProvider {
    fn chain_info(&self) -> Result<ChainInfo> {
        Ok(ChainInfo {
            best_hash: Default::default(),
            best_number: 0,
            last_finalized: None,
            safe_finalized: None,
        })
    }

    fn block(&self, _id: BlockId) -> Result<Option<Block>> {
        Ok(None)
    }

    fn block_number(&self, _hash: H256) -> Result<Option<BlockNumber>> {
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

    fn bytecode_by_hash(&self, _code_hash: H256) -> Result<Option<Bytes>> {
        Ok(None)
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
