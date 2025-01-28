use alloy_primitives::{Address, B256, U256};
use revm::{
    primitives::{AccountInfo, Bytecode},
    Database,
};

/// An enum type that can hold either of two different [`Database`] implementations.
///
/// This allows flexible usage of different [`Database`] types in the same context.
#[derive(Debug, Clone)]
pub enum Either<L, R> {
    /// A value of type `L`.
    Left(L),
    /// A value of type `R`.
    Right(R),
}

impl<L, R> Database for Either<L, R>
where
    L: Database,
    R: Database<Error = L::Error>,
{
    type Error = L::Error;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        match self {
            Self::Left(db) => db.basic(address),
            Self::Right(db) => db.basic(address),
        }
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        match self {
            Self::Left(db) => db.code_by_hash(code_hash),
            Self::Right(db) => db.code_by_hash(code_hash),
        }
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        match self {
            Self::Left(db) => db.storage(address, index),
            Self::Right(db) => db.storage(address, index),
        }
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        match self {
            Self::Left(db) => db.block_hash(number),
            Self::Right(db) => db.block_hash(number),
        }
    }
}
