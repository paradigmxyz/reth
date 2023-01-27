use crate::{AccountProvider, BlockHashProvider, BlockProvider, HeaderProvider, StateProvider};
use parking_lot::Mutex;
use reth_interfaces::Result;
use reth_primitives::{
    keccak256,
    rpc::{BlockId, BlockNumber},
    Account, Address, Block, BlockHash, Bytes, ChainInfo, Header, StorageKey, StorageValue, H256,
    U256,
};
use std::{collections::HashMap, sync::Arc};

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
    bytecode: Option<Bytes>,
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
        self.bytecode = Some(bytecode);
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
}

impl BlockHashProvider for MockEthProvider {
    fn block_hash(&self, number: U256) -> Result<Option<H256>> {
        let lock = self.blocks.lock();

        let hash =
            lock.iter().find_map(
                |(hash, b)| {
                    if b.number == number.to::<u64>() {
                        Some(*hash)
                    } else {
                        None
                    }
                },
            );
        Ok(hash)
    }
}

impl BlockProvider for MockEthProvider {
    fn chain_info(&self) -> Result<ChainInfo> {
        todo!()
    }

    fn block(&self, id: BlockId) -> Result<Option<Block>> {
        let lock = self.blocks.lock();
        match id {
            BlockId::Hash(hash) => Ok(lock.get(&H256(hash.0)).cloned()),
            BlockId::Number(BlockNumber::Number(num)) => {
                Ok(lock.values().find(|b| b.number == num.as_u64()).cloned())
            }
            _ => {
                unreachable!("unused in network tests")
            }
        }
    }

    fn block_number(&self, hash: H256) -> Result<Option<reth_primitives::BlockNumber>> {
        let lock = self.blocks.lock();
        let num = lock.iter().find_map(|(h, b)| if *h == hash { Some(b.number) } else { None });
        Ok(num)
    }
}

impl AccountProvider for MockEthProvider {
    fn basic_account(&self, address: Address) -> Result<Option<Account>> {
        Ok(self.accounts.lock().get(&address).cloned().map(|a| a.account))
    }
}

impl StateProvider for MockEthProvider {
    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytes>> {
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

    fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>> {
        let lock = self.accounts.lock();
        Ok(lock.get(&account).and_then(|account| account.storage.get(&storage_key)).cloned())
    }
}
