use crate::{
    bundle_state::BundleStateWithReceipts,
    traits::{BlockSource, ReceiptProvider},
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader, BlockReaderIdExt,
    BundleStateDataProvider, ChainSpecProvider, ChangeSetReader, EvmEnvProvider, HeaderProvider,
    ReceiptProviderIdExt, StateProvider, StateProviderBox, StateProviderFactory, StateRootProvider,
    TransactionVariant, TransactionsProvider, WithdrawalsProvider,
};
use parking_lot::Mutex;
use reth_db::models::{AccountBeforeTx, StoredBlockBodyIndices};
use reth_interfaces::provider::{ProviderError, ProviderResult};
use reth_primitives::{
    keccak256, trie::AccountProof, Account, Address, Block, BlockHash, BlockHashOrNumber, BlockId,
    BlockNumber, BlockWithSenders, Bytecode, Bytes, ChainInfo, ChainSpec, Header, Receipt,
    SealedBlock, SealedBlockWithSenders, SealedHeader, StorageKey, StorageValue, TransactionMeta,
    TransactionSigned, TransactionSignedNoHash, TxHash, TxNumber, B256, U256,
};
use reth_trie::updates::TrieUpdates;
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
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        let lock = self.headers.lock();
        Ok(lock.get(block_hash).cloned())
    }

    fn header_by_number(&self, num: u64) -> ProviderResult<Option<Header>> {
        let lock = self.headers.lock();
        Ok(lock.values().find(|h| h.number == num).cloned())
    }

    fn header_td(&self, hash: &BlockHash) -> ProviderResult<Option<U256>> {
        let lock = self.headers.lock();
        Ok(lock.get(hash).map(|target| {
            lock.values()
                .filter(|h| h.number < target.number)
                .fold(target.difficulty, |td, h| td + h.difficulty)
        }))
    }

    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>> {
        let lock = self.headers.lock();
        let sum = lock
            .values()
            .filter(|h| h.number <= number)
            .fold(U256::ZERO, |td, h| td + h.difficulty);
        Ok(Some(sum))
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>> {
        let lock = self.headers.lock();

        let mut headers: Vec<_> =
            lock.values().filter(|header| range.contains(&header.number)).cloned().collect();
        headers.sort_by_key(|header| header.number);

        Ok(headers)
    }

    fn sealed_header(&self, number: BlockNumber) -> ProviderResult<Option<SealedHeader>> {
        Ok(self.header_by_number(number)?.map(|h| h.seal_slow()))
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        mut predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        Ok(self
            .headers_range(range)?
            .into_iter()
            .map(|h| h.seal_slow())
            .take_while(|h| predicate(h))
            .collect())
    }
}

impl ChainSpecProvider for MockEthProvider {
    fn chain_spec(&self) -> Arc<ChainSpec> {
        self.chain_spec.clone()
    }
}

impl TransactionsProvider for MockEthProvider {
    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        let lock = self.blocks.lock();
        let tx_number = lock
            .values()
            .flat_map(|block| &block.body)
            .position(|tx| tx.hash() == tx_hash)
            .map(|pos| pos as TxNumber);

        Ok(tx_number)
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<TransactionSigned>> {
        let lock = self.blocks.lock();
        let transaction = lock.values().flat_map(|block| &block.body).nth(id as usize).cloned();

        Ok(transaction)
    }

    fn transaction_by_id_no_hash(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<TransactionSignedNoHash>> {
        let lock = self.blocks.lock();
        let transaction = lock
            .values()
            .flat_map(|block| &block.body)
            .nth(id as usize)
            .map(|tx| Into::<TransactionSignedNoHash>::into(tx.clone()));

        Ok(transaction)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<TransactionSigned>> {
        Ok(self
            .blocks
            .lock()
            .iter()
            .find_map(|(_, block)| block.body.iter().find(|tx| tx.hash() == hash).cloned()))
    }

    fn transaction_by_hash_with_meta(
        &self,
        hash: TxHash,
    ) -> ProviderResult<Option<(TransactionSigned, TransactionMeta)>> {
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

    fn transaction_block(&self, id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
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
    ) -> ProviderResult<Option<Vec<TransactionSigned>>> {
        Ok(self.block(id)?.map(|b| b.body))
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<reth_primitives::BlockNumber>,
    ) -> ProviderResult<Vec<Vec<TransactionSigned>>> {
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
    ) -> ProviderResult<Vec<reth_primitives::TransactionSignedNoHash>> {
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

    fn senders_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
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

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        self.transaction_by_id(id).map(|tx_option| tx_option.map(|tx| tx.recover_signer().unwrap()))
    }
}

impl ReceiptProvider for MockEthProvider {
    fn receipt(&self, _id: TxNumber) -> ProviderResult<Option<Receipt>> {
        Ok(None)
    }

    fn receipt_by_hash(&self, _hash: TxHash) -> ProviderResult<Option<Receipt>> {
        Ok(None)
    }

    fn receipts_by_block(&self, _block: BlockHashOrNumber) -> ProviderResult<Option<Vec<Receipt>>> {
        Ok(None)
    }

    fn receipts_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Receipt>> {
        Ok(vec![])
    }
}

impl ReceiptProviderIdExt for MockEthProvider {}

impl BlockHashReader for MockEthProvider {
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        let lock = self.blocks.lock();

        let hash = lock.iter().find_map(|(hash, b)| (b.number == number).then_some(*hash));
        Ok(hash)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        let range = start..end;
        let lock = self.blocks.lock();

        let mut hashes: Vec<_> =
            lock.iter().filter(|(_, block)| range.contains(&block.number)).collect();
        hashes.sort_by_key(|(_, block)| block.number);

        Ok(hashes.into_iter().map(|(hash, _)| *hash).collect())
    }
}

impl BlockNumReader for MockEthProvider {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        let best_block_number = self.best_block_number()?;
        let lock = self.headers.lock();

        Ok(lock
            .iter()
            .find(|(_, header)| header.number == best_block_number)
            .map(|(hash, header)| ChainInfo { best_hash: *hash, best_number: header.number })
            .unwrap_or_default())
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        let lock = self.headers.lock();
        lock.iter()
            .max_by_key(|h| h.1.number)
            .map(|(_, header)| header.number)
            .ok_or(ProviderError::BestBlockNotFound)
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        self.best_block_number()
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<reth_primitives::BlockNumber>> {
        let lock = self.blocks.lock();
        let num = lock.iter().find_map(|(h, b)| (*h == hash).then_some(b.number));
        Ok(num)
    }
}

impl BlockIdReader for MockEthProvider {
    fn pending_block_num_hash(&self) -> ProviderResult<Option<reth_primitives::BlockNumHash>> {
        Ok(None)
    }

    fn safe_block_num_hash(&self) -> ProviderResult<Option<reth_primitives::BlockNumHash>> {
        Ok(None)
    }

    fn finalized_block_num_hash(&self) -> ProviderResult<Option<reth_primitives::BlockNumHash>> {
        Ok(None)
    }
}

impl BlockReader for MockEthProvider {
    fn find_block_by_hash(
        &self,
        hash: B256,
        _source: BlockSource,
    ) -> ProviderResult<Option<Block>> {
        self.block(hash.into())
    }

    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Block>> {
        let lock = self.blocks.lock();
        match id {
            BlockHashOrNumber::Hash(hash) => Ok(lock.get(&hash).cloned()),
            BlockHashOrNumber::Number(num) => Ok(lock.values().find(|b| b.number == num).cloned()),
        }
    }

    fn pending_block(&self) -> ProviderResult<Option<SealedBlock>> {
        Ok(None)
    }

    fn pending_block_with_senders(&self) -> ProviderResult<Option<SealedBlockWithSenders>> {
        Ok(None)
    }

    fn pending_block_and_receipts(&self) -> ProviderResult<Option<(SealedBlock, Vec<Receipt>)>> {
        Ok(None)
    }

    fn ommers(&self, _id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Header>>> {
        Ok(None)
    }

    fn block_body_indices(&self, _num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        Ok(None)
    }

    fn block_with_senders(
        &self,
        _id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<BlockWithSenders>> {
        Ok(None)
    }

    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Block>> {
        let lock = self.blocks.lock();

        let mut blocks: Vec<_> =
            lock.values().filter(|block| range.contains(&block.number)).cloned().collect();
        blocks.sort_by_key(|block| block.number);

        Ok(blocks)
    }
}

impl BlockReaderIdExt for MockEthProvider {
    fn block_by_id(&self, id: BlockId) -> ProviderResult<Option<Block>> {
        match id {
            BlockId::Number(num) => self.block_by_number_or_tag(num),
            BlockId::Hash(hash) => self.block_by_hash(hash.block_hash),
        }
    }

    fn sealed_header_by_id(&self, id: BlockId) -> ProviderResult<Option<SealedHeader>> {
        self.header_by_id(id)?.map_or_else(|| Ok(None), |h| Ok(Some(h.seal_slow())))
    }

    fn header_by_id(&self, id: BlockId) -> ProviderResult<Option<Header>> {
        match self.block_by_id(id)? {
            None => Ok(None),
            Some(block) => Ok(Some(block.header)),
        }
    }

    fn ommers_by_id(&self, id: BlockId) -> ProviderResult<Option<Vec<Header>>> {
        match id {
            BlockId::Number(num) => self.ommers_by_number_or_tag(num),
            BlockId::Hash(hash) => self.ommers(BlockHashOrNumber::Hash(hash.block_hash)),
        }
    }
}

impl AccountReader for MockEthProvider {
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        Ok(self.accounts.lock().get(&address).cloned().map(|a| a.account))
    }
}

impl StateRootProvider for MockEthProvider {
    fn state_root(&self, _bundle_state: &BundleStateWithReceipts) -> ProviderResult<B256> {
        Ok(B256::default())
    }

    fn state_root_with_updates(
        &self,
        _bundle_state: &BundleStateWithReceipts,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        Ok((B256::default(), Default::default()))
    }
}

impl StateProvider for MockEthProvider {
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        let lock = self.accounts.lock();
        Ok(lock.get(&account).and_then(|account| account.storage.get(&storage_key)).cloned())
    }

    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
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

    fn proof(&self, _address: Address, _keys: &[B256]) -> ProviderResult<AccountProof> {
        Ok(AccountProof::default())
    }
}

impl EvmEnvProvider for MockEthProvider {
    fn fill_env_at(
        &self,
        _cfg: &mut CfgEnv,
        _block_env: &mut BlockEnv,
        _at: BlockHashOrNumber,
    ) -> ProviderResult<()> {
        Ok(())
    }

    fn fill_env_with_header(
        &self,
        _cfg: &mut CfgEnv,
        _block_env: &mut BlockEnv,
        _header: &Header,
    ) -> ProviderResult<()> {
        Ok(())
    }

    fn fill_block_env_at(
        &self,
        _block_env: &mut BlockEnv,
        _at: BlockHashOrNumber,
    ) -> ProviderResult<()> {
        Ok(())
    }

    fn fill_block_env_with_header(
        &self,
        _block_env: &mut BlockEnv,
        _header: &Header,
    ) -> ProviderResult<()> {
        Ok(())
    }

    fn fill_cfg_env_at(&self, _cfg: &mut CfgEnv, _at: BlockHashOrNumber) -> ProviderResult<()> {
        Ok(())
    }

    fn fill_cfg_env_with_header(&self, _cfg: &mut CfgEnv, _header: &Header) -> ProviderResult<()> {
        Ok(())
    }
}

impl StateProviderFactory for MockEthProvider {
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn history_by_block_number(&self, _block: BlockNumber) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn history_by_block_hash(&self, _block: BlockHash) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn state_by_block_hash(&self, _block: BlockHash) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn pending(&self) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn pending_state_by_hash(&self, _block_hash: B256) -> ProviderResult<Option<StateProviderBox>> {
        Ok(Some(Box::new(self.clone())))
    }

    fn pending_with_provider<'a>(
        &'a self,
        _bundle_state_data: Box<dyn BundleStateDataProvider + 'a>,
    ) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }
}

impl StateProviderFactory for Arc<MockEthProvider> {
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn history_by_block_number(&self, _block: BlockNumber) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn history_by_block_hash(&self, _block: BlockHash) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn state_by_block_hash(&self, _block: BlockHash) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn pending(&self) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn pending_state_by_hash(&self, _block_hash: B256) -> ProviderResult<Option<StateProviderBox>> {
        Ok(Some(Box::new(self.clone())))
    }

    fn pending_with_provider<'a>(
        &'a self,
        _bundle_state_data: Box<dyn BundleStateDataProvider + 'a>,
    ) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }
}

impl WithdrawalsProvider for MockEthProvider {
    fn latest_withdrawal(&self) -> ProviderResult<Option<reth_primitives::Withdrawal>> {
        Ok(None)
    }
    fn withdrawals_by_block(
        &self,
        _id: BlockHashOrNumber,
        _timestamp: u64,
    ) -> ProviderResult<Option<Vec<reth_primitives::Withdrawal>>> {
        Ok(None)
    }
}

impl ChangeSetReader for MockEthProvider {
    fn account_block_changeset(
        &self,
        _block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>> {
        Ok(Vec::default())
    }
}
