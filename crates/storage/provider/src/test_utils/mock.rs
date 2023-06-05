use crate::{
    traits::{BlockSource, ReceiptProvider},
    AccountProvider, BlockHashProvider, BlockIdProvider, BlockNumProvider, BlockProvider,
    BlockProviderIdExt, EvmEnvProvider, HeaderProvider, PostState, PostStateDataProvider,
    StateProvider, StateProviderBox, StateProviderFactory, StateRootProvider, TransactionsProvider,
};
use parking_lot::Mutex;
use reth_db::models::StoredBlockBodyIndices;
use reth_interfaces::{provider::ProviderError, Result};
use reth_primitives::{
    keccak256, Account, Address, Block, BlockHash, BlockHashOrNumber, BlockId, BlockNumber,
    Bytecode, Bytes, ChainInfo, Header, Receipt, SealedBlock, SealedHeader, StorageKey,
    StorageValue, TransactionMeta, TransactionSigned, TxHash, TxNumber, H256, U256,
};
use reth_revm_primitives::primitives::{BlockEnv, CfgEnv};
use std::{
    collections::{BTreeMap, HashMap},
    ops::RangeBounds,
    sync::Arc,
};

/// A mock implementation for Provider interfaces.
#[derive(Debug, Clone, Default)]
pub struct MockEthProvider {
    /// Local block store
    pub blocks: Arc<Mutex<HashMap<H256, Block>>>,
    /// Local header store
    pub headers: Arc<Mutex<HashMap<H256, Header>>>,
    /// Local account store
    pub accounts: Arc<Mutex<HashMap<Address, ExtendedAccount>>>,
}

/// An extended account for local store
#[derive(Debug, Clone)]
pub struct ExtendedAccount {
    account: Account,
    bytecode: Option<Bytecode>,
    storage: HashMap<StorageKey, StorageValue>,
}

impl ExtendedAccount {
    /// Create new instance of extended account
    pub fn new(nonce: u64, balance: U256) -> Self {
        Self {
            account: Account { nonce, balance, bytecode_hash: None },
            bytecode: None,
            storage: Default::default(),
        }
    }

    /// Set bytecode and bytecode hash on the extended account
    pub fn with_bytecode(mut self, bytecode: Bytes) -> Self {
        let hash = keccak256(&bytecode);
        self.account.bytecode_hash = Some(hash);
        self.bytecode = Some(Bytecode::new_raw(bytecode.into()));
        self
    }

    /// Add storage to the extended account. If the storage key is already present,
    /// the value is updated.
    pub fn extend_storage(
        mut self,
        storage: impl IntoIterator<Item = (StorageKey, StorageValue)>,
    ) -> Self {
        self.storage.extend(storage);
        self
    }
}

impl MockEthProvider {
    /// Add block to local block store
    pub fn add_block(&self, hash: H256, block: Block) {
        self.add_header(hash, block.header.clone());
        self.blocks.lock().insert(hash, block);
    }

    /// Add multiple blocks to local block store
    pub fn extend_blocks(&self, iter: impl IntoIterator<Item = (H256, Block)>) {
        for (hash, block) in iter.into_iter() {
            self.add_header(hash, block.header.clone());
            self.add_block(hash, block)
        }
    }

    /// Add header to local header store
    pub fn add_header(&self, hash: H256, header: Header) {
        self.headers.lock().insert(hash, header);
    }

    /// Add multiple headers to local header store
    pub fn extend_headers(&self, iter: impl IntoIterator<Item = (H256, Header)>) {
        for (hash, header) in iter.into_iter() {
            self.add_header(hash, header)
        }
    }

    /// Add account to local account store
    pub fn add_account(&self, address: Address, account: ExtendedAccount) {
        self.accounts.lock().insert(address, account);
    }

    /// Add account to local account store
    pub fn extend_accounts(&self, iter: impl IntoIterator<Item = (Address, ExtendedAccount)>) {
        for (address, account) in iter.into_iter() {
            self.add_account(address, account)
        }
    }
}

impl HeaderProvider for MockEthProvider {
    fn header(&self, block_hash: &BlockHash) -> Result<Option<Header>> {
        let lock = self.headers.lock();
        Ok(lock.get(block_hash).cloned())
    }

    fn header_by_number(&self, num: u64) -> Result<Option<Header>> {
        let lock = self.headers.lock();
        Ok(lock.values().find(|h| h.number == num).cloned())
    }

    fn header_td(&self, hash: &BlockHash) -> Result<Option<U256>> {
        let lock = self.headers.lock();
        Ok(lock.get(hash).map(|target| {
            lock.values()
                .filter(|h| h.number < target.number)
                .fold(target.difficulty, |td, h| td + h.difficulty)
        }))
    }

    fn header_td_by_number(&self, number: BlockNumber) -> Result<Option<U256>> {
        let lock = self.headers.lock();
        let sum = lock
            .values()
            .filter(|h| h.number <= number)
            .fold(U256::ZERO, |td, h| td + h.difficulty);
        Ok(Some(sum))
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> Result<Vec<Header>> {
        let lock = self.headers.lock();

        let mut headers: Vec<_> =
            lock.values().filter(|header| range.contains(&header.number)).cloned().collect();
        headers.sort_by_key(|header| header.number);

        Ok(headers)
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<SealedHeader>> {
        Ok(self.headers_range(range)?.into_iter().map(|h| h.seal_slow()).collect())
    }

    fn sealed_header(&self, number: BlockNumber) -> Result<Option<SealedHeader>> {
        Ok(self.header_by_number(number)?.map(|h| h.seal_slow()))
    }
}

impl TransactionsProvider for MockEthProvider {
    fn transaction_id(&self, _tx_hash: TxHash) -> Result<Option<TxNumber>> {
        todo!()
    }

    fn transaction_by_id(&self, _id: TxNumber) -> Result<Option<TransactionSigned>> {
        Ok(None)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> Result<Option<TransactionSigned>> {
        Ok(self
            .blocks
            .lock()
            .iter()
            .find_map(|(_, block)| block.body.iter().find(|tx| tx.hash() == hash).cloned()))
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: TxHash,
    ) -> Result<Option<(TransactionSigned, TransactionMeta)>> {
        Ok(None)
    }

    fn transaction_block(&self, _id: TxNumber) -> Result<Option<BlockNumber>> {
        unimplemented!()
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> Result<Option<Vec<TransactionSigned>>> {
        Ok(self.block(id)?.map(|b| b.body))
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<reth_primitives::BlockNumber>,
    ) -> Result<Vec<Vec<TransactionSigned>>> {
        // init btreemap so we can return in order
        let mut map = BTreeMap::new();
        for (_, block) in self.blocks.lock().iter() {
            if range.contains(&block.number) {
                map.insert(block.number, block.body.clone());
            }
        }

        Ok(map.into_values().collect())
    }
}

impl ReceiptProvider for MockEthProvider {
    fn receipt(&self, _id: TxNumber) -> Result<Option<Receipt>> {
        Ok(None)
    }

    fn receipt_by_hash(&self, _hash: TxHash) -> Result<Option<Receipt>> {
        Ok(None)
    }

    fn receipts_by_block(&self, _block: BlockHashOrNumber) -> Result<Option<Vec<Receipt>>> {
        Ok(None)
    }
}

impl BlockHashProvider for MockEthProvider {
    fn block_hash(&self, number: u64) -> Result<Option<H256>> {
        let lock = self.blocks.lock();

        let hash = lock.iter().find_map(|(hash, b)| (b.number == number).then_some(*hash));
        Ok(hash)
    }

    fn canonical_hashes_range(&self, start: BlockNumber, end: BlockNumber) -> Result<Vec<H256>> {
        let range = start..end;
        let lock = self.blocks.lock();

        let mut hashes: Vec<_> =
            lock.iter().filter(|(_, block)| range.contains(&block.number)).collect();
        hashes.sort_by_key(|(_, block)| block.number);

        Ok(hashes.into_iter().map(|(hash, _)| *hash).collect())
    }
}

impl BlockNumProvider for MockEthProvider {
    fn chain_info(&self) -> Result<ChainInfo> {
        let best_block_number = self.best_block_number()?;
        let lock = self.headers.lock();

        Ok(lock
            .iter()
            .find(|(_, header)| header.number == best_block_number)
            .map(|(hash, header)| ChainInfo { best_hash: *hash, best_number: header.number })
            .unwrap_or_default())
    }

    fn best_block_number(&self) -> Result<BlockNumber> {
        let lock = self.headers.lock();
        Ok(lock
            .iter()
            .max_by_key(|h| h.1.number)
            .map(|(_, header)| header.number)
            .ok_or(ProviderError::BestBlockNotFound)?)
    }

    fn last_block_number(&self) -> Result<BlockNumber> {
        self.best_block_number()
    }

    fn block_number(&self, hash: H256) -> Result<Option<reth_primitives::BlockNumber>> {
        let lock = self.blocks.lock();
        let num = lock.iter().find_map(|(h, b)| (*h == hash).then_some(b.number));
        Ok(num)
    }
}

impl BlockIdProvider for MockEthProvider {
    fn pending_block_num_hash(&self) -> Result<Option<reth_primitives::BlockNumHash>> {
        Ok(None)
    }

    fn safe_block_num_hash(&self) -> Result<Option<reth_primitives::BlockNumHash>> {
        Ok(None)
    }

    fn finalized_block_num_hash(&self) -> Result<Option<reth_primitives::BlockNumHash>> {
        Ok(None)
    }
}

impl BlockProvider for MockEthProvider {
    fn find_block_by_hash(&self, hash: H256, _source: BlockSource) -> Result<Option<Block>> {
        self.block(hash.into())
    }

    fn block(&self, id: BlockHashOrNumber) -> Result<Option<Block>> {
        let lock = self.blocks.lock();
        match id {
            BlockHashOrNumber::Hash(hash) => Ok(lock.get(&hash).cloned()),
            BlockHashOrNumber::Number(num) => Ok(lock.values().find(|b| b.number == num).cloned()),
        }
    }

    fn pending_block(&self) -> Result<Option<SealedBlock>> {
        Ok(None)
    }

    fn pending_header(&self) -> Result<Option<SealedHeader>> {
        Ok(None)
    }

    fn ommers(&self, _id: BlockHashOrNumber) -> Result<Option<Vec<Header>>> {
        Ok(None)
    }

    fn block_body_indices(&self, _num: u64) -> Result<Option<StoredBlockBodyIndices>> {
        Ok(None)
    }
}

impl BlockProviderIdExt for MockEthProvider {
    fn block_by_id(&self, id: BlockId) -> Result<Option<Block>> {
        match id {
            BlockId::Number(num) => self.block_by_number_or_tag(num),
            BlockId::Hash(hash) => self.block_by_hash(hash.block_hash),
        }
    }

    fn sealed_header_by_id(&self, id: BlockId) -> Result<Option<SealedHeader>> {
        self.header_by_id(id)?.map_or_else(|| Ok(None), |h| Ok(Some(h.seal_slow())))
    }

    fn header_by_id(&self, id: BlockId) -> Result<Option<Header>> {
        match self.block_by_id(id)? {
            None => Ok(None),
            Some(block) => Ok(Some(block.header)),
        }
    }

    fn ommers_by_id(&self, id: BlockId) -> Result<Option<Vec<Header>>> {
        match id {
            BlockId::Number(num) => self.ommers_by_number_or_tag(num),
            BlockId::Hash(hash) => self.ommers(BlockHashOrNumber::Hash(hash.block_hash)),
        }
    }
}

impl AccountProvider for MockEthProvider {
    fn basic_account(&self, address: Address) -> Result<Option<Account>> {
        Ok(self.accounts.lock().get(&address).cloned().map(|a| a.account))
    }
}

impl StateRootProvider for MockEthProvider {
    fn state_root(&self, _post_state: PostState) -> Result<H256> {
        todo!()
    }
}

impl StateProvider for MockEthProvider {
    fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>> {
        let lock = self.accounts.lock();
        Ok(lock.get(&account).and_then(|account| account.storage.get(&storage_key)).cloned())
    }

    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytecode>> {
        let lock = self.accounts.lock();
        Ok(lock.values().find_map(|account| {
            match (account.account.bytecode_hash.as_ref(), account.bytecode.as_ref()) {
                (Some(bytecode_hash), Some(bytecode)) if *bytecode_hash == code_hash => {
                    Some(bytecode.clone())
                }
                _ => None,
            }
        }))
    }

    fn proof(
        &self,
        _address: Address,
        _keys: &[H256],
    ) -> Result<(Vec<Bytes>, H256, Vec<Vec<Bytes>>)> {
        todo!()
    }
}

impl EvmEnvProvider for MockEthProvider {
    fn fill_env_at(
        &self,
        _cfg: &mut CfgEnv,
        _block_env: &mut BlockEnv,
        _at: BlockHashOrNumber,
    ) -> Result<()> {
        unimplemented!()
    }

    fn fill_env_with_header(
        &self,
        _cfg: &mut CfgEnv,
        _block_env: &mut BlockEnv,
        _header: &Header,
    ) -> Result<()> {
        unimplemented!()
    }

    fn fill_block_env_at(&self, _block_env: &mut BlockEnv, _at: BlockHashOrNumber) -> Result<()> {
        unimplemented!()
    }

    fn fill_block_env_with_header(
        &self,
        _block_env: &mut BlockEnv,
        _header: &Header,
    ) -> Result<()> {
        unimplemented!()
    }

    fn fill_cfg_env_at(&self, _cfg: &mut CfgEnv, _at: BlockHashOrNumber) -> Result<()> {
        unimplemented!()
    }

    fn fill_cfg_env_with_header(&self, _cfg: &mut CfgEnv, _header: &Header) -> Result<()> {
        unimplemented!()
    }
}

impl StateProviderFactory for MockEthProvider {
    fn latest(&self) -> Result<StateProviderBox<'_>> {
        Ok(Box::new(self.clone()))
    }

    fn history_by_block_number(&self, _block: BlockNumber) -> Result<StateProviderBox<'_>> {
        todo!()
    }

    fn history_by_block_hash(&self, _block: BlockHash) -> Result<StateProviderBox<'_>> {
        todo!()
    }

    fn state_by_block_hash(&self, _block: BlockHash) -> Result<StateProviderBox<'_>> {
        todo!()
    }

    fn pending(&self) -> Result<StateProviderBox<'_>> {
        todo!()
    }

    fn pending_with_provider<'a>(
        &'a self,
        _post_state_data: Box<dyn PostStateDataProvider + 'a>,
    ) -> Result<StateProviderBox<'a>> {
        todo!()
    }
}

impl StateProviderFactory for Arc<MockEthProvider> {
    fn latest(&self) -> Result<StateProviderBox<'_>> {
        Ok(Box::new(self.clone()))
    }

    fn history_by_block_number(&self, _block: BlockNumber) -> Result<StateProviderBox<'_>> {
        todo!()
    }

    fn history_by_block_hash(&self, _block: BlockHash) -> Result<StateProviderBox<'_>> {
        todo!()
    }

    fn state_by_block_hash(&self, _block: BlockHash) -> Result<StateProviderBox<'_>> {
        todo!()
    }

    fn pending(&self) -> Result<StateProviderBox<'_>> {
        todo!()
    }

    fn pending_with_provider<'a>(
        &'a self,
        _post_state_data: Box<dyn PostStateDataProvider + 'a>,
    ) -> Result<StateProviderBox<'a>> {
        todo!()
    }
}
