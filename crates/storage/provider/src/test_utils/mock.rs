use crate::{
    bundle_state::BundleStateWithReceipts,
    traits::{BlockSource, ReceiptProvider},
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader, BlockReaderIdExt,
    BundleStateDataProvider, ChainSpecProvider, EvmEnvProvider, HeaderProvider,
    ReceiptProviderIdExt, StateProvider, StateProviderBox, StateProviderFactory, StateRootProvider,
    TransactionsProvider, WithdrawalsProvider,
};
use parking_lot::Mutex;
use reth_db::models::StoredBlockBodyIndices;
use reth_interfaces::{provider::ProviderError, RethResult};
use reth_primitives::{
    keccak256, Account, Address, Block, BlockHash, BlockHashOrNumber, BlockId, BlockNumber,
    BlockWithSenders, Bytecode, Bytes, ChainInfo, ChainSpec, Header, Receipt, SealedBlock,
    SealedHeader, StorageKey, StorageValue, TransactionMeta, TransactionSigned,
    TransactionSignedNoHash, TxHash, TxNumber, B256, U256,
};
use revm::primitives::{BlockEnv, CfgEnv};
use std::{
    collections::{BTreeMap, HashMap},
    ops::{RangeBounds, RangeInclusive},
    sync::Arc,
};

/// A mock implementation for Provider interfaces.
#[derive(Debug, Clone)]
pub struct MockEthProvider {
    /// Local block store
    pub blocks: Arc<Mutex<HashMap<B256, Block>>>,
    /// Local header store
    pub headers: Arc<Mutex<HashMap<B256, Header>>>,
    /// Local account store
    pub accounts: Arc<Mutex<HashMap<Address, ExtendedAccount>>>,
    /// Local chain spec
    pub chain_spec: Arc<ChainSpec>,
}

impl Default for MockEthProvider {
    fn default() -> MockEthProvider {
        MockEthProvider {
            blocks: Default::default(),
            headers: Default::default(),
            accounts: Default::default(),
            chain_spec: Arc::new(reth_primitives::ChainSpecBuilder::mainnet().build()),
        }
    }
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
        self.bytecode = Some(Bytecode::new_raw(bytecode));
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
    pub fn add_block(&self, hash: B256, block: Block) {
        self.add_header(hash, block.header.clone());
        self.blocks.lock().insert(hash, block);
    }

    /// Add multiple blocks to local block store
    pub fn extend_blocks(&self, iter: impl IntoIterator<Item = (B256, Block)>) {
        for (hash, block) in iter.into_iter() {
            self.add_header(hash, block.header.clone());
            self.add_block(hash, block)
        }
    }

    /// Add header to local header store
    pub fn add_header(&self, hash: B256, header: Header) {
        self.headers.lock().insert(hash, header);
    }

    /// Add multiple headers to local header store
    pub fn extend_headers(&self, iter: impl IntoIterator<Item = (B256, Header)>) {
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
    fn header(&self, block_hash: &BlockHash) -> RethResult<Option<Header>> {
        let lock = self.headers.lock();
        Ok(lock.get(block_hash).cloned())
    }

    fn header_by_number(&self, num: u64) -> RethResult<Option<Header>> {
        let lock = self.headers.lock();
        Ok(lock.values().find(|h| h.number == num).cloned())
    }

    fn header_td(&self, hash: &BlockHash) -> RethResult<Option<U256>> {
        let lock = self.headers.lock();
        Ok(lock.get(hash).map(|target| {
            lock.values()
                .filter(|h| h.number < target.number)
                .fold(target.difficulty, |td, h| td + h.difficulty)
        }))
    }

    fn header_td_by_number(&self, number: BlockNumber) -> RethResult<Option<U256>> {
        let lock = self.headers.lock();
        let sum = lock
            .values()
            .filter(|h| h.number <= number)
            .fold(U256::ZERO, |td, h| td + h.difficulty);
        Ok(Some(sum))
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> RethResult<Vec<Header>> {
        let lock = self.headers.lock();

        let mut headers: Vec<_> =
            lock.values().filter(|header| range.contains(&header.number)).cloned().collect();
        headers.sort_by_key(|header| header.number);

        Ok(headers)
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> RethResult<Vec<SealedHeader>> {
        Ok(self.headers_range(range)?.into_iter().map(|h| h.seal_slow()).collect())
    }

    fn sealed_header(&self, number: BlockNumber) -> RethResult<Option<SealedHeader>> {
        Ok(self.header_by_number(number)?.map(|h| h.seal_slow()))
    }
}

impl ChainSpecProvider for MockEthProvider {
    fn chain_spec(&self) -> Arc<ChainSpec> {
        self.chain_spec.clone()
    }
}

impl TransactionsProvider for MockEthProvider {
    fn transaction_id(&self, tx_hash: TxHash) -> RethResult<Option<TxNumber>> {
        let lock = self.blocks.lock();
        let tx_number = lock
            .values()
            .flat_map(|block| &block.body)
            .position(|tx| tx.hash() == tx_hash)
            .map(|pos| pos as TxNumber);

        Ok(tx_number)
    }

    fn transaction_by_id(&self, id: TxNumber) -> RethResult<Option<TransactionSigned>> {
        let lock = self.blocks.lock();
        let transaction = lock.values().flat_map(|block| &block.body).nth(id as usize).cloned();

        Ok(transaction)
    }

    fn transaction_by_id_no_hash(
        &self,
        id: TxNumber,
    ) -> RethResult<Option<TransactionSignedNoHash>> {
        let lock = self.blocks.lock();
        let transaction = lock
            .values()
            .flat_map(|block| &block.body)
            .nth(id as usize)
            .map(|tx| Into::<TransactionSignedNoHash>::into(tx.clone()));

        Ok(transaction)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> RethResult<Option<TransactionSigned>> {
        Ok(self
            .blocks
            .lock()
            .iter()
            .find_map(|(_, block)| block.body.iter().find(|tx| tx.hash() == hash).cloned()))
    }

    fn transaction_by_hash_with_meta(
        &self,
        hash: TxHash,
    ) -> RethResult<Option<(TransactionSigned, TransactionMeta)>> {
        let lock = self.blocks.lock();
        for (block_hash, block) in lock.iter() {
            for (index, tx) in block.body.iter().enumerate() {
                if tx.hash() == hash {
                    let meta = TransactionMeta {
                        tx_hash: hash,
                        index: index as u64,
                        block_hash: *block_hash,
                        block_number: block.header.number,
                        base_fee: block.header.base_fee_per_gas,
                        excess_blob_gas: block.header.excess_blob_gas,
                    };
                    return Ok(Some((tx.clone(), meta)))
                }
            }
        }
        Ok(None)
    }

    fn transaction_block(&self, id: TxNumber) -> RethResult<Option<BlockNumber>> {
        let lock = self.blocks.lock();
        let mut current_tx_number: TxNumber = 0;
        for block in lock.values() {
            if current_tx_number + (block.body.len() as TxNumber) > id {
                return Ok(Some(block.header.number))
            }
            current_tx_number += block.body.len() as TxNumber;
        }
        Ok(None)
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> RethResult<Option<Vec<TransactionSigned>>> {
        Ok(self.block(id)?.map(|b| b.body))
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<reth_primitives::BlockNumber>,
    ) -> RethResult<Vec<Vec<TransactionSigned>>> {
        // init btreemap so we can return in order
        let mut map = BTreeMap::new();
        for (_, block) in self.blocks.lock().iter() {
            if range.contains(&block.number) {
                map.insert(block.number, block.body.clone());
            }
        }

        Ok(map.into_values().collect())
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> RethResult<Vec<reth_primitives::TransactionSignedNoHash>> {
        let lock = self.blocks.lock();
        let transactions = lock
            .values()
            .flat_map(|block| &block.body)
            .enumerate()
            .filter_map(|(tx_number, tx)| {
                if range.contains(&(tx_number as TxNumber)) {
                    Some(tx.clone().into())
                } else {
                    None
                }
            })
            .collect();

        Ok(transactions)
    }

    fn senders_by_tx_range(&self, range: impl RangeBounds<TxNumber>) -> RethResult<Vec<Address>> {
        let lock = self.blocks.lock();
        let transactions = lock
            .values()
            .flat_map(|block| &block.body)
            .enumerate()
            .filter_map(|(tx_number, tx)| {
                if range.contains(&(tx_number as TxNumber)) {
                    Some(tx.recover_signer()?)
                } else {
                    None
                }
            })
            .collect();

        Ok(transactions)
    }

    fn transaction_sender(&self, id: TxNumber) -> RethResult<Option<Address>> {
        self.transaction_by_id(id).map(|tx_option| tx_option.map(|tx| tx.recover_signer().unwrap()))
    }
}

impl ReceiptProvider for MockEthProvider {
    fn receipt(&self, _id: TxNumber) -> RethResult<Option<Receipt>> {
        Ok(None)
    }

    fn receipt_by_hash(&self, _hash: TxHash) -> RethResult<Option<Receipt>> {
        Ok(None)
    }

    fn receipts_by_block(&self, _block: BlockHashOrNumber) -> RethResult<Option<Vec<Receipt>>> {
        Ok(None)
    }
}

impl ReceiptProviderIdExt for MockEthProvider {}

impl BlockHashReader for MockEthProvider {
    fn block_hash(&self, number: u64) -> RethResult<Option<B256>> {
        let lock = self.blocks.lock();

        let hash = lock.iter().find_map(|(hash, b)| (b.number == number).then_some(*hash));
        Ok(hash)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> RethResult<Vec<B256>> {
        let range = start..end;
        let lock = self.blocks.lock();

        let mut hashes: Vec<_> =
            lock.iter().filter(|(_, block)| range.contains(&block.number)).collect();
        hashes.sort_by_key(|(_, block)| block.number);

        Ok(hashes.into_iter().map(|(hash, _)| *hash).collect())
    }
}

impl BlockNumReader for MockEthProvider {
    fn chain_info(&self) -> RethResult<ChainInfo> {
        let best_block_number = self.best_block_number()?;
        let lock = self.headers.lock();

        Ok(lock
            .iter()
            .find(|(_, header)| header.number == best_block_number)
            .map(|(hash, header)| ChainInfo { best_hash: *hash, best_number: header.number })
            .unwrap_or_default())
    }

    fn best_block_number(&self) -> RethResult<BlockNumber> {
        let lock = self.headers.lock();
        Ok(lock
            .iter()
            .max_by_key(|h| h.1.number)
            .map(|(_, header)| header.number)
            .ok_or(ProviderError::BestBlockNotFound)?)
    }

    fn last_block_number(&self) -> RethResult<BlockNumber> {
        self.best_block_number()
    }

    fn block_number(&self, hash: B256) -> RethResult<Option<reth_primitives::BlockNumber>> {
        let lock = self.blocks.lock();
        let num = lock.iter().find_map(|(h, b)| (*h == hash).then_some(b.number));
        Ok(num)
    }
}

impl BlockIdReader for MockEthProvider {
    fn pending_block_num_hash(&self) -> RethResult<Option<reth_primitives::BlockNumHash>> {
        Ok(None)
    }

    fn safe_block_num_hash(&self) -> RethResult<Option<reth_primitives::BlockNumHash>> {
        Ok(None)
    }

    fn finalized_block_num_hash(&self) -> RethResult<Option<reth_primitives::BlockNumHash>> {
        Ok(None)
    }
}

impl BlockReader for MockEthProvider {
    fn find_block_by_hash(&self, hash: B256, _source: BlockSource) -> RethResult<Option<Block>> {
        self.block(hash.into())
    }

    fn block(&self, id: BlockHashOrNumber) -> RethResult<Option<Block>> {
        let lock = self.blocks.lock();
        match id {
            BlockHashOrNumber::Hash(hash) => Ok(lock.get(&hash).cloned()),
            BlockHashOrNumber::Number(num) => Ok(lock.values().find(|b| b.number == num).cloned()),
        }
    }

    fn pending_block(&self) -> RethResult<Option<SealedBlock>> {
        Ok(None)
    }

    fn pending_block_and_receipts(&self) -> RethResult<Option<(SealedBlock, Vec<Receipt>)>> {
        Ok(None)
    }

    fn ommers(&self, _id: BlockHashOrNumber) -> RethResult<Option<Vec<Header>>> {
        Ok(None)
    }

    fn block_body_indices(&self, _num: u64) -> RethResult<Option<StoredBlockBodyIndices>> {
        Ok(None)
    }

    fn block_with_senders(&self, _number: BlockNumber) -> RethResult<Option<BlockWithSenders>> {
        Ok(None)
    }

    fn block_range(&self, _range: RangeInclusive<BlockNumber>) -> RethResult<Vec<Block>> {
        Ok(vec![])
    }
}

impl BlockReaderIdExt for MockEthProvider {
    fn block_by_id(&self, id: BlockId) -> RethResult<Option<Block>> {
        match id {
            BlockId::Number(num) => self.block_by_number_or_tag(num),
            BlockId::Hash(hash) => self.block_by_hash(hash.block_hash),
        }
    }

    fn sealed_header_by_id(&self, id: BlockId) -> RethResult<Option<SealedHeader>> {
        self.header_by_id(id)?.map_or_else(|| Ok(None), |h| Ok(Some(h.seal_slow())))
    }

    fn header_by_id(&self, id: BlockId) -> RethResult<Option<Header>> {
        match self.block_by_id(id)? {
            None => Ok(None),
            Some(block) => Ok(Some(block.header)),
        }
    }

    fn ommers_by_id(&self, id: BlockId) -> RethResult<Option<Vec<Header>>> {
        match id {
            BlockId::Number(num) => self.ommers_by_number_or_tag(num),
            BlockId::Hash(hash) => self.ommers(BlockHashOrNumber::Hash(hash.block_hash)),
        }
    }
}

impl AccountReader for MockEthProvider {
    fn basic_account(&self, address: Address) -> RethResult<Option<Account>> {
        Ok(self.accounts.lock().get(&address).cloned().map(|a| a.account))
    }
}

impl StateRootProvider for MockEthProvider {
    fn state_root(&self, _state: &BundleStateWithReceipts) -> RethResult<B256> {
        todo!()
    }
}

impl StateProvider for MockEthProvider {
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> RethResult<Option<StorageValue>> {
        let lock = self.accounts.lock();
        Ok(lock.get(&account).and_then(|account| account.storage.get(&storage_key)).cloned())
    }

    fn bytecode_by_hash(&self, code_hash: B256) -> RethResult<Option<Bytecode>> {
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
        _keys: &[B256],
    ) -> RethResult<(Vec<Bytes>, B256, Vec<Vec<Bytes>>)> {
        todo!()
    }
}

impl EvmEnvProvider for MockEthProvider {
    fn fill_env_at(
        &self,
        _cfg: &mut CfgEnv,
        _block_env: &mut BlockEnv,
        _at: BlockHashOrNumber,
    ) -> RethResult<()> {
        unimplemented!()
    }

    fn fill_env_with_header(
        &self,
        _cfg: &mut CfgEnv,
        _block_env: &mut BlockEnv,
        _header: &Header,
    ) -> RethResult<()> {
        unimplemented!()
    }

    fn fill_block_env_at(
        &self,
        _block_env: &mut BlockEnv,
        _at: BlockHashOrNumber,
    ) -> RethResult<()> {
        unimplemented!()
    }

    fn fill_block_env_with_header(
        &self,
        _block_env: &mut BlockEnv,
        _header: &Header,
    ) -> RethResult<()> {
        unimplemented!()
    }

    fn fill_cfg_env_at(&self, _cfg: &mut CfgEnv, _at: BlockHashOrNumber) -> RethResult<()> {
        unimplemented!()
    }

    fn fill_cfg_env_with_header(&self, _cfg: &mut CfgEnv, _header: &Header) -> RethResult<()> {
        unimplemented!()
    }
}

impl StateProviderFactory for MockEthProvider {
    fn latest(&self) -> RethResult<StateProviderBox<'_>> {
        Ok(Box::new(self.clone()))
    }

    fn history_by_block_number(&self, _block: BlockNumber) -> RethResult<StateProviderBox<'_>> {
        Ok(Box::new(self.clone()))
    }

    fn history_by_block_hash(&self, _block: BlockHash) -> RethResult<StateProviderBox<'_>> {
        Ok(Box::new(self.clone()))
    }

    fn state_by_block_hash(&self, _block: BlockHash) -> RethResult<StateProviderBox<'_>> {
        Ok(Box::new(self.clone()))
    }

    fn pending(&self) -> RethResult<StateProviderBox<'_>> {
        Ok(Box::new(self.clone()))
    }

    fn pending_state_by_hash(&self, _block_hash: B256) -> RethResult<Option<StateProviderBox<'_>>> {
        Ok(Some(Box::new(self.clone())))
    }

    fn pending_with_provider<'a>(
        &'a self,
        _post_state_data: Box<dyn BundleStateDataProvider + 'a>,
    ) -> RethResult<StateProviderBox<'a>> {
        Ok(Box::new(self.clone()))
    }
}

impl StateProviderFactory for Arc<MockEthProvider> {
    fn latest(&self) -> RethResult<StateProviderBox<'_>> {
        Ok(Box::new(self.clone()))
    }

    fn history_by_block_number(&self, _block: BlockNumber) -> RethResult<StateProviderBox<'_>> {
        Ok(Box::new(self.clone()))
    }

    fn history_by_block_hash(&self, _block: BlockHash) -> RethResult<StateProviderBox<'_>> {
        Ok(Box::new(self.clone()))
    }

    fn state_by_block_hash(&self, _block: BlockHash) -> RethResult<StateProviderBox<'_>> {
        Ok(Box::new(self.clone()))
    }

    fn pending(&self) -> RethResult<StateProviderBox<'_>> {
        Ok(Box::new(self.clone()))
    }

    fn pending_state_by_hash(&self, _block_hash: B256) -> RethResult<Option<StateProviderBox<'_>>> {
        Ok(Some(Box::new(self.clone())))
    }

    fn pending_with_provider<'a>(
        &'a self,
        _post_state_data: Box<dyn BundleStateDataProvider + 'a>,
    ) -> RethResult<StateProviderBox<'a>> {
        Ok(Box::new(self.clone()))
    }
}

impl WithdrawalsProvider for MockEthProvider {
    fn latest_withdrawal(&self) -> RethResult<Option<reth_primitives::Withdrawal>> {
        unimplemented!()
    }
    fn withdrawals_by_block(
        &self,
        _id: BlockHashOrNumber,
        _timestamp: u64,
    ) -> RethResult<Option<Vec<reth_primitives::Withdrawal>>> {
        unimplemented!()
    }
}
