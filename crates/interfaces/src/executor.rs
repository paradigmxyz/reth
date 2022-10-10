use async_trait::async_trait;
use reth_primitives::{
    Account, Address, Block, BlockNumber, Bytes, StorageKey, StorageValue, H256, U256,
};
use thiserror::Error;

/// Takes block and executes it, returns error
#[async_trait]
pub trait BlockExecutor {
    /// Execute block
    async fn execute(&self, _block: Block) -> Error {
        Error::VerificationFailed
    }
}

/// BlockExecutor Errors
#[derive(Error, Debug, Clone)]
pub enum Error {
    /// Example of error
    #[error("Example of error.")]
    VerificationFailed,
}

/// Function needed for executor
pub trait ExecutorDb {
    /// Get Block by BlockNumber.
    fn block(&self, _height: BlockNumber) -> Option<Block>;

    /// Get storage.
    fn storage(&self, account: Address, storage_key: StorageKey) -> Option<StorageValue>;

    /// Get basic account information.
    fn basic_account(&self, adderss: Address) -> Option<Account>;

    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: H256) -> Option<(Bytes, usize)>;

    /// Get block hash by number.
    fn block_hash(&self, number: U256) -> Option<H256>;
}
