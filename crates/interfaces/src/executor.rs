use async_trait::async_trait;
use reth_primitives::{Address, Block, BlockNumber, StorageKey, StorageValue};
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
    fn get_block(&self, _height: BlockNumber) -> Option<Block>;

    /// Get storage.
    fn get_storage(&self, account: Address, storage_key: StorageKey) -> Option<StorageValue>;
}
