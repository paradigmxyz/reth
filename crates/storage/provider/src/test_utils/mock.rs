use crate::{
    traits::{BlockSource, ReceiptProvider},
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader, BlockReaderIdExt,
    ChainSpecProvider, ChangeSetReader, HeaderProvider, PruneCheckpointReader,
    ReceiptProviderIdExt, StateProvider, StateProviderBox, StateProviderFactory, StateReader,
    StateRootProvider, TransactionVariant, TransactionsProvider,
};
use alloy_consensus::{
    constants::EMPTY_ROOT_HASH,
    transaction::{TransactionMeta, TxHashRef},
    BlockHeader,
};
use alloy_eips::{BlockHashOrNumber, BlockId, BlockNumberOrTag};
use alloy_primitives::{
    keccak256, map::HashMap, Address, BlockHash, BlockNumber, Bytes, StorageKey, StorageValue,
    TxHash, TxNumber, B256, U256,
};
use parking_lot::Mutex;
use reth_chain_state::{CanonStateNotifications, CanonStateSubscriptions};
use reth_chainspec::{ChainInfo, EthChainSpec};
use reth_db::transaction::DbTx;
use reth_db_api::{
    mock::{DatabaseMock, TxMock},
    models::{AccountBeforeTx, StoredBlockBodyIndices},
};
use reth_ethereum_primitives::EthPrimitives;
use reth_execution_types::ExecutionOutcome;
use reth_primitives_traits::{
    Account, Block, BlockBody, Bytecode, GotExpected, NodePrimitives, RecoveredBlock, SealedHeader,
    SignerRecoverable, StorageEntry,
};
use reth_prune_types::{PruneCheckpoint, PruneModes, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{
    BlockBodyIndicesProvider, BytecodeReader, DBProvider, DatabaseProviderFactory,
    HashedPostStateProvider, NodePrimitivesProvider, StageCheckpointReader, StateProofProvider,
    StorageChangeSetReader, StorageRootProvider, TrieReader,
};
use reth_storage_errors::provider::{ConsistentViewError, ProviderError, ProviderResult};
use reth_trie::{
    updates::{TrieUpdates, TrieUpdatesSorted},
    AccountProof, HashedPostState, HashedStorage, MultiProof, MultiProofTargets, StorageMultiProof,
    StorageProof, TrieInput,
};
use std::{
    collections::BTreeMap,
    fmt::Debug,
    ops::{RangeBounds, RangeInclusive},
    sync::Arc,
};
use tokio::sync::broadcast;

/// A mock implementation for Provider interfaces.
#[derive(Debug)]
pub struct MockEthProvider<T: NodePrimitives = EthPrimitives, ChainSpec = reth_chainspec::ChainSpec>
{
    ///local block store
    pub blocks: Arc<Mutex<HashMap<B256, T::Block>>>,
    /// Local header store
    pub headers: Arc<Mutex<HashMap<B256, <T::Block as Block>::Header>>>,
    /// Local receipt store indexed by block number
    pub receipts: Arc<Mutex<HashMap<BlockNumber, Vec<T::Receipt>>>>,
    /// Local account store
    pub accounts: Arc<Mutex<HashMap<Address, ExtendedAccount>>>,
    /// Local chain spec
    pub chain_spec: Arc<ChainSpec>,
    /// Local state roots
    pub state_roots: Arc<Mutex<Vec<B256>>>,
    /// Local block body indices store
    pub block_body_indices: Arc<Mutex<HashMap<BlockNumber, StoredBlockBodyIndices>>>,
    tx: TxMock,
    prune_modes: Arc<PruneModes>,
}

impl<T: NodePrimitives, ChainSpec> Clone for MockEthProvider<T, ChainSpec>
where
    T::Block: Clone,
{
    fn clone(&self) -> Self {
        Self {
            blocks: self.blocks.clone(),
            headers: self.headers.clone(),
            receipts: self.receipts.clone(),
            accounts: self.accounts.clone(),
            chain_spec: self.chain_spec.clone(),
            state_roots: self.state_roots.clone(),
            block_body_indices: self.block_body_indices.clone(),
            tx: self.tx.clone(),
            prune_modes: self.prune_modes.clone(),
        }
    }
}

impl<T: NodePrimitives> MockEthProvider<T, reth_chainspec::ChainSpec> {
    /// Create a new, empty instance
    pub fn new() -> Self {
        Self {
            blocks: Default::default(),
            headers: Default::default(),
            receipts: Default::default(),
            accounts: Default::default(),
            chain_spec: Arc::new(reth_chainspec::ChainSpecBuilder::mainnet().build()),
            state_roots: Default::default(),
            block_body_indices: Default::default(),
            tx: Default::default(),
            prune_modes: Default::default(),
        }
    }
}

impl<T: NodePrimitives, ChainSpec> MockEthProvider<T, ChainSpec> {
    /// Add block to local block store
    pub fn add_block(&self, hash: B256, block: T::Block) {
        self.add_header(hash, block.header().clone());
        self.blocks.lock().insert(hash, block);
    }

    /// Add multiple blocks to local block store
    pub fn extend_blocks(&self, iter: impl IntoIterator<Item = (B256, T::Block)>) {
        for (hash, block) in iter {
            self.add_block(hash, block)
        }
    }

    /// Add header to local header store
    pub fn add_header(&self, hash: B256, header: <T::Block as Block>::Header) {
        self.headers.lock().insert(hash, header);
    }

    /// Add multiple headers to local header store
    pub fn extend_headers(
        &self,
        iter: impl IntoIterator<Item = (B256, <T::Block as Block>::Header)>,
    ) {
        for (hash, header) in iter {
            self.add_header(hash, header)
        }
    }

    /// Add account to local account store
    pub fn add_account(&self, address: Address, account: ExtendedAccount) {
        self.accounts.lock().insert(address, account);
    }

    /// Add account to local account store
    pub fn extend_accounts(&self, iter: impl IntoIterator<Item = (Address, ExtendedAccount)>) {
        for (address, account) in iter {
            self.add_account(address, account)
        }
    }

    /// Add receipts to local receipt store
    pub fn add_receipts(&self, block_number: BlockNumber, receipts: Vec<T::Receipt>) {
        self.receipts.lock().insert(block_number, receipts);
    }

    /// Add multiple receipts to local receipt store
    pub fn extend_receipts(&self, iter: impl IntoIterator<Item = (BlockNumber, Vec<T::Receipt>)>) {
        for (block_number, receipts) in iter {
            self.add_receipts(block_number, receipts);
        }
    }

    /// Add block body indices to local store
    pub fn add_block_body_indices(
        &self,
        block_number: BlockNumber,
        indices: StoredBlockBodyIndices,
    ) {
        self.block_body_indices.lock().insert(block_number, indices);
    }

    /// Add state root to local state root store
    pub fn add_state_root(&self, state_root: B256) {
        self.state_roots.lock().push(state_root);
    }

    /// Set chain spec.
    pub fn with_chain_spec<C>(self, chain_spec: C) -> MockEthProvider<T, C> {
        MockEthProvider {
            blocks: self.blocks,
            headers: self.headers,
            receipts: self.receipts,
            accounts: self.accounts,
            chain_spec: Arc::new(chain_spec),
            state_roots: self.state_roots,
            block_body_indices: self.block_body_indices,
            tx: self.tx,
            prune_modes: self.prune_modes,
        }
    }
}

impl Default for MockEthProvider {
    fn default() -> Self {
        Self::new()
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

impl<T: NodePrimitives, ChainSpec: EthChainSpec + Clone + 'static> DatabaseProviderFactory
    for MockEthProvider<T, ChainSpec>
{
    type DB = DatabaseMock;
    type Provider = Self;
    type ProviderRW = Self;

    fn database_provider_ro(&self) -> ProviderResult<Self::Provider> {
        Err(ConsistentViewError::Syncing { best_block: GotExpected::new(0, 0) }.into())
    }

    fn database_provider_rw(&self) -> ProviderResult<Self::ProviderRW> {
        Err(ConsistentViewError::Syncing { best_block: GotExpected::new(0, 0) }.into())
    }
}

impl<T: NodePrimitives, ChainSpec: EthChainSpec + 'static> DBProvider
    for MockEthProvider<T, ChainSpec>
{
    type Tx = TxMock;

    fn tx_ref(&self) -> &Self::Tx {
        &self.tx
    }

    fn tx_mut(&mut self) -> &mut Self::Tx {
        &mut self.tx
    }

    fn into_tx(self) -> Self::Tx {
        self.tx
    }

    fn commit(self) -> ProviderResult<bool> {
        Ok(self.tx.commit()?)
    }

    fn prune_modes_ref(&self) -> &PruneModes {
        &self.prune_modes
    }
}

impl<T: NodePrimitives, ChainSpec: EthChainSpec + Send + Sync + 'static> HeaderProvider
    for MockEthProvider<T, ChainSpec>
{
    type Header = <T::Block as Block>::Header;

    fn header(&self, block_hash: BlockHash) -> ProviderResult<Option<Self::Header>> {
        let lock = self.headers.lock();
        Ok(lock.get(&block_hash).cloned())
    }

    fn header_by_number(&self, num: u64) -> ProviderResult<Option<Self::Header>> {
        let lock = self.headers.lock();
        Ok(lock.values().find(|h| h.number() == num).cloned())
    }

    fn headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::Header>> {
        let lock = self.headers.lock();

        let mut headers: Vec<_> =
            lock.values().filter(|header| range.contains(&header.number())).cloned().collect();
        headers.sort_by_key(|header| header.number());

        Ok(headers)
    }

    fn sealed_header(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        Ok(self.header_by_number(number)?.map(SealedHeader::seal_slow))
    }

    fn sealed_headers_while(
        &self,
        range: impl RangeBounds<BlockNumber>,
        mut predicate: impl FnMut(&SealedHeader<Self::Header>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        Ok(self
            .headers_range(range)?
            .into_iter()
            .map(SealedHeader::seal_slow)
            .take_while(|h| predicate(h))
            .collect())
    }
}

impl<T, ChainSpec> ChainSpecProvider for MockEthProvider<T, ChainSpec>
where
    T: NodePrimitives,
    ChainSpec: EthChainSpec + 'static + Debug + Send + Sync,
{
    type ChainSpec = ChainSpec;

    fn chain_spec(&self) -> Arc<Self::ChainSpec> {
        self.chain_spec.clone()
    }
}

impl<T: NodePrimitives, ChainSpec: EthChainSpec + 'static> TransactionsProvider
    for MockEthProvider<T, ChainSpec>
{
    type Transaction = T::SignedTx;

    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        let lock = self.blocks.lock();
        let tx_number = lock
            .values()
            .flat_map(|block| block.body().transactions())
            .position(|tx| *tx.tx_hash() == tx_hash)
            .map(|pos| pos as TxNumber);

        Ok(tx_number)
    }

    fn transaction_by_id(&self, id: TxNumber) -> ProviderResult<Option<Self::Transaction>> {
        let lock = self.blocks.lock();
        let transaction =
            lock.values().flat_map(|block| block.body().transactions()).nth(id as usize).cloned();

        Ok(transaction)
    }

    fn transaction_by_id_unhashed(
        &self,
        id: TxNumber,
    ) -> ProviderResult<Option<Self::Transaction>> {
        let lock = self.blocks.lock();
        let transaction =
            lock.values().flat_map(|block| block.body().transactions()).nth(id as usize).cloned();

        Ok(transaction)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Transaction>> {
        Ok(self.blocks.lock().iter().find_map(|(_, block)| {
            block.body().transactions_iter().find(|tx| *tx.tx_hash() == hash).cloned()
        }))
    }

    fn transaction_by_hash_with_meta(
        &self,
        hash: TxHash,
    ) -> ProviderResult<Option<(Self::Transaction, TransactionMeta)>> {
        let lock = self.blocks.lock();
        for (block_hash, block) in lock.iter() {
            for (index, tx) in block.body().transactions_iter().enumerate() {
                if *tx.tx_hash() == hash {
                    let meta = TransactionMeta {
                        tx_hash: hash,
                        index: index as u64,
                        block_hash: *block_hash,
                        block_number: block.header().number(),
                        base_fee: block.header().base_fee_per_gas(),
                        excess_blob_gas: block.header().excess_blob_gas(),
                        timestamp: block.header().timestamp(),
                    };
                    return Ok(Some((tx.clone(), meta)))
                }
            }
        }
        Ok(None)
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Transaction>>> {
        Ok(self.block(id)?.map(|b| b.body().clone_transactions()))
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<alloy_primitives::BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Transaction>>> {
        // init btreemap so we can return in order
        let mut map = BTreeMap::new();
        for (_, block) in self.blocks.lock().iter() {
            if range.contains(&block.header().number()) {
                map.insert(block.header().number(), block.body().clone_transactions());
            }
        }

        Ok(map.into_values().collect())
    }

    fn transactions_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Transaction>> {
        let lock = self.blocks.lock();
        let transactions = lock
            .values()
            .flat_map(|block| block.body().transactions())
            .enumerate()
            .filter(|&(tx_number, _)| range.contains(&(tx_number as TxNumber)))
            .map(|(_, tx)| tx.clone())
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
            .flat_map(|block| block.body().transactions())
            .enumerate()
            .filter_map(|(tx_number, tx)| {
                if range.contains(&(tx_number as TxNumber)) {
                    tx.recover_signer().ok()
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

impl<T, ChainSpec> ReceiptProvider for MockEthProvider<T, ChainSpec>
where
    T: NodePrimitives,
    ChainSpec: Send + Sync + 'static,
{
    type Receipt = T::Receipt;

    fn receipt(&self, _id: TxNumber) -> ProviderResult<Option<Self::Receipt>> {
        Ok(None)
    }

    fn receipt_by_hash(&self, _hash: TxHash) -> ProviderResult<Option<Self::Receipt>> {
        Ok(None)
    }

    fn receipts_by_block(
        &self,
        block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        let receipts_lock = self.receipts.lock();

        match block {
            BlockHashOrNumber::Hash(hash) => {
                // Find block number by hash first
                let headers_lock = self.headers.lock();
                if let Some(header) = headers_lock.get(&hash) {
                    Ok(receipts_lock.get(&header.number()).cloned())
                } else {
                    Ok(None)
                }
            }
            BlockHashOrNumber::Number(number) => Ok(receipts_lock.get(&number).cloned()),
        }
    }

    fn receipts_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Receipt>> {
        Ok(vec![])
    }

    fn receipts_by_block_range(
        &self,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Receipt>>> {
        let receipts_lock = self.receipts.lock();
        let headers_lock = self.headers.lock();

        let mut result = Vec::new();
        for block_number in block_range {
            // Only include blocks that exist in headers (i.e., have been added to the provider)
            if headers_lock.values().any(|header| header.number() == block_number) {
                if let Some(block_receipts) = receipts_lock.get(&block_number) {
                    result.push(block_receipts.clone());
                } else {
                    // If block exists but no receipts found, add empty vec
                    result.push(vec![]);
                }
            }
        }

        Ok(result)
    }
}

impl<T, ChainSpec> ReceiptProviderIdExt for MockEthProvider<T, ChainSpec>
where
    T: NodePrimitives,
    Self: ReceiptProvider + BlockIdReader,
{
}

impl<T: NodePrimitives, ChainSpec: Send + Sync + 'static> BlockHashReader
    for MockEthProvider<T, ChainSpec>
{
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        let lock = self.headers.lock();
        let hash =
            lock.iter().find_map(|(hash, header)| (header.number() == number).then_some(*hash));
        Ok(hash)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        let lock = self.headers.lock();
        let mut hashes: Vec<_> =
            lock.iter().filter(|(_, header)| (start..end).contains(&header.number())).collect();

        hashes.sort_by_key(|(_, header)| header.number());

        Ok(hashes.into_iter().map(|(hash, _)| *hash).collect())
    }
}

impl<T: NodePrimitives, ChainSpec: Send + Sync + 'static> BlockNumReader
    for MockEthProvider<T, ChainSpec>
{
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        let best_block_number = self.best_block_number()?;
        let lock = self.headers.lock();

        Ok(lock
            .iter()
            .find(|(_, header)| header.number() == best_block_number)
            .map(|(hash, header)| ChainInfo { best_hash: *hash, best_number: header.number() })
            .unwrap_or_default())
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        let lock = self.headers.lock();
        lock.iter()
            .max_by_key(|h| h.1.number())
            .map(|(_, header)| header.number())
            .ok_or(ProviderError::BestBlockNotFound)
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        self.best_block_number()
    }

    fn block_number(&self, hash: B256) -> ProviderResult<Option<alloy_primitives::BlockNumber>> {
        let lock = self.headers.lock();
        Ok(lock.get(&hash).map(|header| header.number()))
    }
}

impl<T: NodePrimitives, ChainSpec: EthChainSpec + Send + Sync + 'static> BlockIdReader
    for MockEthProvider<T, ChainSpec>
{
    fn pending_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        Ok(None)
    }

    fn safe_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        Ok(None)
    }

    fn finalized_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        Ok(None)
    }
}

//look
impl<T: NodePrimitives, ChainSpec: EthChainSpec + Send + Sync + 'static> BlockReader
    for MockEthProvider<T, ChainSpec>
{
    type Block = T::Block;

    fn find_block_by_hash(
        &self,
        hash: B256,
        _source: BlockSource,
    ) -> ProviderResult<Option<Self::Block>> {
        self.block(hash.into())
    }

    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>> {
        let lock = self.blocks.lock();
        match id {
            BlockHashOrNumber::Hash(hash) => Ok(lock.get(&hash).cloned()),
            BlockHashOrNumber::Number(num) => {
                Ok(lock.values().find(|b| b.header().number() == num).cloned())
            }
        }
    }

    fn pending_block(&self) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        Ok(None)
    }

    fn pending_block_and_receipts(
        &self,
    ) -> ProviderResult<Option<(RecoveredBlock<Self::Block>, Vec<T::Receipt>)>> {
        Ok(None)
    }

    fn recovered_block(
        &self,
        _id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        Ok(None)
    }

    fn sealed_block_with_senders(
        &self,
        _id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        Ok(None)
    }

    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Self::Block>> {
        let lock = self.blocks.lock();

        let mut blocks: Vec<_> = lock
            .values()
            .filter(|block| range.contains(&block.header().number()))
            .cloned()
            .collect();
        blocks.sort_by_key(|block| block.header().number());

        Ok(blocks)
    }

    fn block_with_senders_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        Ok(vec![])
    }

    fn recovered_block_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        Ok(vec![])
    }

    fn block_by_transaction_id(&self, _id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        Ok(None)
    }
}

impl<T, ChainSpec> BlockReaderIdExt for MockEthProvider<T, ChainSpec>
where
    ChainSpec: EthChainSpec + Send + Sync + 'static,
    T: NodePrimitives,
{
    fn block_by_id(&self, id: BlockId) -> ProviderResult<Option<T::Block>> {
        match id {
            BlockId::Number(num) => self.block_by_number_or_tag(num),
            BlockId::Hash(hash) => self.block_by_hash(hash.block_hash),
        }
    }

    fn sealed_header_by_id(
        &self,
        id: BlockId,
    ) -> ProviderResult<Option<SealedHeader<<T::Block as Block>::Header>>> {
        self.header_by_id(id)?.map_or_else(|| Ok(None), |h| Ok(Some(SealedHeader::seal_slow(h))))
    }

    fn header_by_id(&self, id: BlockId) -> ProviderResult<Option<<T::Block as Block>::Header>> {
        match self.block_by_id(id)? {
            None => Ok(None),
            Some(block) => Ok(Some(block.into_header())),
        }
    }
}

impl<T: NodePrimitives, ChainSpec: Send + Sync> AccountReader for MockEthProvider<T, ChainSpec> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        Ok(self.accounts.lock().get(address).cloned().map(|a| a.account))
    }
}

impl<T: NodePrimitives, ChainSpec: Send + Sync> StageCheckpointReader
    for MockEthProvider<T, ChainSpec>
{
    fn get_stage_checkpoint(&self, _id: StageId) -> ProviderResult<Option<StageCheckpoint>> {
        Ok(None)
    }

    fn get_stage_checkpoint_progress(&self, _id: StageId) -> ProviderResult<Option<Vec<u8>>> {
        Ok(None)
    }

    fn get_all_checkpoints(&self) -> ProviderResult<Vec<(String, StageCheckpoint)>> {
        Ok(vec![])
    }
}

impl<T: NodePrimitives, ChainSpec: Send + Sync> PruneCheckpointReader
    for MockEthProvider<T, ChainSpec>
{
    fn get_prune_checkpoint(
        &self,
        _segment: PruneSegment,
    ) -> ProviderResult<Option<PruneCheckpoint>> {
        Ok(None)
    }

    fn get_prune_checkpoints(&self) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>> {
        Ok(vec![])
    }
}

impl<T, ChainSpec> StateRootProvider for MockEthProvider<T, ChainSpec>
where
    T: NodePrimitives,
    ChainSpec: Send + Sync,
{
    fn state_root(&self, _state: HashedPostState) -> ProviderResult<B256> {
        Ok(self.state_roots.lock().pop().unwrap_or_default())
    }

    fn state_root_from_nodes(&self, _input: TrieInput) -> ProviderResult<B256> {
        Ok(self.state_roots.lock().pop().unwrap_or_default())
    }

    fn state_root_with_updates(
        &self,
        _state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let state_root = self.state_roots.lock().pop().unwrap_or_default();
        Ok((state_root, Default::default()))
    }

    fn state_root_from_nodes_with_updates(
        &self,
        _input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let state_root = self.state_roots.lock().pop().unwrap_or_default();
        Ok((state_root, Default::default()))
    }
}

impl<T, ChainSpec> StorageRootProvider for MockEthProvider<T, ChainSpec>
where
    T: NodePrimitives,
    ChainSpec: Send + Sync,
{
    fn storage_root(
        &self,
        _address: Address,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        Ok(EMPTY_ROOT_HASH)
    }

    fn storage_proof(
        &self,
        _address: Address,
        slot: B256,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageProof> {
        Ok(StorageProof::new(slot))
    }

    fn storage_multiproof(
        &self,
        _address: Address,
        _slots: &[B256],
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        Ok(StorageMultiProof::empty())
    }
}

impl<T, ChainSpec> StateProofProvider for MockEthProvider<T, ChainSpec>
where
    T: NodePrimitives,
    ChainSpec: Send + Sync,
{
    fn proof(
        &self,
        _input: TrieInput,
        address: Address,
        _slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        Ok(AccountProof::new(address))
    }

    fn multiproof(
        &self,
        _input: TrieInput,
        _targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        Ok(MultiProof::default())
    }

    fn witness(&self, _input: TrieInput, _target: HashedPostState) -> ProviderResult<Vec<Bytes>> {
        Ok(Vec::default())
    }
}

impl<T: NodePrimitives, ChainSpec: EthChainSpec + 'static> HashedPostStateProvider
    for MockEthProvider<T, ChainSpec>
{
    fn hashed_post_state(&self, _state: &revm_database::BundleState) -> HashedPostState {
        HashedPostState::default()
    }
}

impl<T, ChainSpec> StateProvider for MockEthProvider<T, ChainSpec>
where
    T: NodePrimitives,
    ChainSpec: EthChainSpec + Send + Sync + 'static,
{
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        let lock = self.accounts.lock();
        Ok(lock.get(&account).and_then(|account| account.storage.get(&storage_key)).copied())
    }
}

impl<T, ChainSpec> BytecodeReader for MockEthProvider<T, ChainSpec>
where
    T: NodePrimitives,
    ChainSpec: Send + Sync,
{
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        let lock = self.accounts.lock();
        Ok(lock.values().find_map(|account| {
            match (account.account.bytecode_hash.as_ref(), account.bytecode.as_ref()) {
                (Some(bytecode_hash), Some(bytecode)) if bytecode_hash == code_hash => {
                    Some(bytecode.clone())
                }
                _ => None,
            }
        }))
    }
}

impl<T: NodePrimitives, ChainSpec: EthChainSpec + Send + Sync + 'static> StateProviderFactory
    for MockEthProvider<T, ChainSpec>
{
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn state_by_block_number_or_tag(
        &self,
        number_or_tag: BlockNumberOrTag,
    ) -> ProviderResult<StateProviderBox> {
        match number_or_tag {
            BlockNumberOrTag::Latest => self.latest(),
            BlockNumberOrTag::Finalized => {
                // we can only get the finalized state by hash, not by num
                let hash =
                    self.finalized_block_hash()?.ok_or(ProviderError::FinalizedBlockNotFound)?;

                // only look at historical state
                self.history_by_block_hash(hash)
            }
            BlockNumberOrTag::Safe => {
                // we can only get the safe state by hash, not by num
                let hash = self.safe_block_hash()?.ok_or(ProviderError::SafeBlockNotFound)?;

                self.history_by_block_hash(hash)
            }
            BlockNumberOrTag::Earliest => {
                self.history_by_block_number(self.earliest_block_number()?)
            }
            BlockNumberOrTag::Pending => self.pending(),
            BlockNumberOrTag::Number(num) => self.history_by_block_number(num),
        }
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

    fn maybe_pending(&self) -> ProviderResult<Option<StateProviderBox>> {
        Ok(Some(Box::new(self.clone())))
    }
}

impl<T: NodePrimitives, ChainSpec: Send + Sync> BlockBodyIndicesProvider
    for MockEthProvider<T, ChainSpec>
{
    fn block_body_indices(&self, num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        Ok(self.block_body_indices.lock().get(&num).copied())
    }
    fn block_body_indices_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<StoredBlockBodyIndices>> {
        Ok(vec![])
    }
}

impl<T: NodePrimitives, ChainSpec: Send + Sync> ChangeSetReader for MockEthProvider<T, ChainSpec> {
    fn account_block_changeset(
        &self,
        _block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>> {
        Ok(Vec::default())
    }

    fn get_account_before_block(
        &self,
        _block_number: BlockNumber,
        _address: Address,
    ) -> ProviderResult<Option<AccountBeforeTx>> {
        Ok(None)
    }

    fn account_changesets_range(
        &self,
        _range: impl core::ops::RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<(BlockNumber, AccountBeforeTx)>> {
        Ok(Vec::default())
    }

    fn account_changeset_count(&self) -> ProviderResult<usize> {
        Ok(0)
    }
}

impl<T: NodePrimitives, ChainSpec: Send + Sync> StorageChangeSetReader
    for MockEthProvider<T, ChainSpec>
{
    fn storage_changeset(
        &self,
        _block_number: BlockNumber,
    ) -> ProviderResult<Vec<(reth_db_api::models::BlockNumberAddress, StorageEntry)>> {
        Ok(Vec::default())
    }

    fn get_storage_before_block(
        &self,
        _block_number: BlockNumber,
        _address: Address,
        _storage_key: B256,
    ) -> ProviderResult<Option<StorageEntry>> {
        Ok(None)
    }

    fn storage_changesets_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<(reth_db_api::models::BlockNumberAddress, StorageEntry)>> {
        Ok(Vec::default())
    }

    fn storage_changeset_count(&self) -> ProviderResult<usize> {
        Ok(0)
    }
}

impl<T: NodePrimitives, ChainSpec: Send + Sync> StateReader for MockEthProvider<T, ChainSpec> {
    type Receipt = T::Receipt;

    fn get_state(
        &self,
        _block: BlockNumber,
    ) -> ProviderResult<Option<ExecutionOutcome<Self::Receipt>>> {
        Ok(None)
    }
}

impl<T: NodePrimitives, ChainSpec: Send + Sync> TrieReader for MockEthProvider<T, ChainSpec> {
    fn trie_reverts(&self, _from: BlockNumber) -> ProviderResult<TrieUpdatesSorted> {
        Ok(TrieUpdatesSorted::default())
    }

    fn get_block_trie_updates(
        &self,
        _block_number: BlockNumber,
    ) -> ProviderResult<TrieUpdatesSorted> {
        Ok(TrieUpdatesSorted::default())
    }
}

impl<T: NodePrimitives, ChainSpec: Send + Sync> CanonStateSubscriptions
    for MockEthProvider<T, ChainSpec>
{
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications<T> {
        broadcast::channel(1).1
    }
}

impl<T: NodePrimitives, ChainSpec: Send + Sync> NodePrimitivesProvider
    for MockEthProvider<T, ChainSpec>
{
    type Primitives = T;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::BlockHash;
    use reth_ethereum_primitives::Receipt;

    #[test]
    fn test_mock_provider_receipts() {
        let provider = MockEthProvider::<EthPrimitives>::new();

        let block_hash = BlockHash::random();
        let block_number = 1u64;
        let header = Header { number: block_number, ..Default::default() };

        let receipt1 = Receipt { cumulative_gas_used: 21000, success: true, ..Default::default() };
        let receipt2 = Receipt { cumulative_gas_used: 42000, success: true, ..Default::default() };
        let receipts = vec![receipt1, receipt2];

        provider.add_header(block_hash, header);
        provider.add_receipts(block_number, receipts.clone());

        let result = provider.receipts_by_block(block_hash.into()).unwrap();
        assert_eq!(result, Some(receipts.clone()));

        let result = provider.receipts_by_block(block_number.into()).unwrap();
        assert_eq!(result, Some(receipts.clone()));

        let range_result = provider.receipts_by_block_range(1..=1).unwrap();
        assert_eq!(range_result, vec![receipts]);

        let non_existent = provider.receipts_by_block(BlockHash::random().into()).unwrap();
        assert_eq!(non_existent, None);

        let empty_range = provider.receipts_by_block_range(10..=20).unwrap();
        assert_eq!(empty_range, Vec::<Vec<Receipt>>::new());
    }

    #[test]
    fn test_mock_provider_receipts_multiple_blocks() {
        let provider = MockEthProvider::<EthPrimitives>::new();

        let block1_hash = BlockHash::random();
        let block2_hash = BlockHash::random();
        let block1_number = 1u64;
        let block2_number = 2u64;

        let header1 = Header { number: block1_number, ..Default::default() };
        let header2 = Header { number: block2_number, ..Default::default() };

        let receipts1 =
            vec![Receipt { cumulative_gas_used: 21000, success: true, ..Default::default() }];
        let receipts2 =
            vec![Receipt { cumulative_gas_used: 42000, success: true, ..Default::default() }];

        provider.add_header(block1_hash, header1);
        provider.add_header(block2_hash, header2);
        provider.add_receipts(block1_number, receipts1.clone());
        provider.add_receipts(block2_number, receipts2.clone());

        let range_result = provider.receipts_by_block_range(1..=2).unwrap();
        assert_eq!(range_result.len(), 2);
        assert_eq!(range_result[0], receipts1);
        assert_eq!(range_result[1], receipts2);

        let partial_range = provider.receipts_by_block_range(1..=1).unwrap();
        assert_eq!(partial_range.len(), 1);
        assert_eq!(partial_range[0], receipts1);
    }
}
