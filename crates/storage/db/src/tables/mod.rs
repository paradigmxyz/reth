//! Tables and data models.
//!
//! # Overview
//!
//! This module defines the tables in reth, as well as some table-related abstractions:
//!
//! - [`codecs`] integrates different codecs into [`Encode`](crate::abstraction::table::Encode) and
//!   [`Decode`](crate::abstraction::table::Decode)
//! - [`models`] defines the values written to tables
//!
//! # Database Tour
//!
//! TODO(onbjerg): Find appropriate format for this...

// TODO: remove when https://github.com/proptest-rs/proptest/pull/427 is merged
#![allow(unknown_lints, non_local_definitions)]

pub mod codecs;
pub mod models;

mod raw;
pub use raw::{RawDupSort, RawKey, RawTable, RawValue, TableRawRow};

pub(crate) mod utils;

use crate::{
    abstraction::table::Table,
    table::DupSort,
    tables::{
        codecs::CompactU256,
        models::{
            accounts::{AccountBeforeTx, BlockNumberAddress},
            blocks::{HeaderHash, StoredBlockOmmers},
            client_version::ClientVersion,
            storage_sharded_key::StorageShardedKey,
            ShardedKey, StoredBlockBodyIndices, StoredBlockWithdrawals,
        },
    },
};
use reth_primitives::{
    stage::StageCheckpoint,
    trie::{StorageTrieEntry, StoredBranchNode, StoredNibbles, StoredNibblesSubKey},
    Account, Address, BlockHash, BlockNumber, Bytecode, Header, IntegerList, PruneCheckpoint,
    PruneSegment, Receipt, StorageEntry, TransactionSignedNoHash, TxHash, TxNumber, B256,
};
use std::fmt;

/// Enum for the types of tables present in libmdbx.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum TableType {
    /// key value table
    Table,
    /// Duplicate key value table
    DupSort,
}

/// The general purpose of this is to use with a combination of Tables enum,
/// by implementing a `TableViewer` trait you can operate on db tables in an abstract way.
///
/// # Example
///
/// ```
/// use reth_db::{
///     table::{DupSort, Table},
///     TableViewer, Tables,
/// };
///
/// struct MyTableViewer;
///
/// impl TableViewer<()> for MyTableViewer {
///     type Error = &'static str;
///
///     fn view<T: Table>(&self) -> Result<(), Self::Error> {
///         // operate on table in a generic way
///         Ok(())
///     }
///
///     fn view_dupsort<T: DupSort>(&self) -> Result<(), Self::Error> {
///         // operate on a dupsort table in a generic way
///         Ok(())
///     }
/// }
///
/// let viewer = MyTableViewer {};
///
/// let _ = Tables::Headers.view(&viewer);
/// let _ = Tables::Transactions.view(&viewer);
/// ```
pub trait TableViewer<R> {
    /// The error type returned by the viewer.
    type Error;

    /// Operate on the table in a generic way.
    fn view<T: Table>(&self) -> Result<R, Self::Error>;

    /// Operate on the dupsort table in a generic way.
    ///
    /// By default, the `view` function is invoked unless overridden.
    fn view_dupsort<T: DupSort>(&self) -> Result<R, Self::Error> {
        self.view::<T>()
    }
}

/// Defines all the tables in the database.
macro_rules! tables {
    (@bool) => { false };
    (@bool $($t:tt)+) => { true };

    (@view $name:ident $v:ident) => { $v.view::<$name>() };
    (@view $name:ident $v:ident $_subkey:ty) => { $v.view_dupsort::<$name>() };

    ($( $(#[$attr:meta])* table $name:ident<Key = $key:ty, Value = $value:ty $(, SubKey = $subkey:ty)? $(,)?>; )*) => {
        // Table marker types.
        $(
            $(#[$attr])*
            ///
            #[doc = concat!("Marker type representing a database table mapping [`", stringify!($key), "`] to [`", stringify!($value), "`].")]
            $(
                #[doc = concat!("\n\nThis table's `DUPSORT` subkey is [`", stringify!($subkey), "`].")]
            )?
            pub struct $name {
                _private: (),
            }

            // Ideally this implementation wouldn't exist, but it is necessary to derive `Debug`
            // when a type is generic over `T: Table`. See: https://github.com/rust-lang/rust/issues/26925
            impl fmt::Debug for $name {
                fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
                    unreachable!("this type cannot be instantiated")
                }
            }

            impl $crate::table::Table for $name {
                const TABLE: Tables = Tables::$name;

                type Key = $key;
                type Value = $value;
            }

            $(
                impl DupSort for $name {
                    type SubKey = $subkey;
                }
            )?
        )*

        // Tables enum.
        // NOTE: the ordering of the enum does not matter, but it is assumed that the discriminants
        // start at 0 and increment by 1 for each variant (the default behavior).
        // See for example `reth_db::implementation::mdbx::tx::Tx::db_handles`.

        /// A table in the database.
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Tables {
            $(
                #[doc = concat!("The [`", stringify!($name), "`] database table.")]
                $name,
            )*
        }

        impl Tables {
            /// All the tables in the database.
            pub const ALL: &'static [Self] = &[$(Self::$name,)*];

            /// The number of tables in the database.
            pub const COUNT: usize = Self::ALL.len();

            /// Returns the name of the table as a string.
            pub const fn name(&self) -> &'static str {
                match self {
                    $(
                        Self::$name => table_names::$name,
                    )*
                }
            }

            /// Returns `true` if the table is a `DUPSORT` table.
            pub const fn is_dupsort(&self) -> bool {
                match self {
                    $(
                        Self::$name => tables!(@bool $($subkey)?),
                    )*
                }
            }

            /// The type of the given table in database.
            pub const fn table_type(&self) -> TableType {
                if self.is_dupsort() {
                    TableType::DupSort
                } else {
                    TableType::Table
                }
            }

            /// Allows to operate on specific table type
            pub fn view<T, R>(&self, visitor: &T) -> Result<R, T::Error>
            where
                T: TableViewer<R>,
            {
                match self {
                    $(
                        Self::$name => tables!(@view $name visitor $($subkey)?),
                    )*
                }
            }
        }

        impl fmt::Debug for Tables {
            #[inline]
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.name())
            }
        }

        impl fmt::Display for Tables {
            #[inline]
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.name().fmt(f)
            }
        }

        impl std::str::FromStr for Tables {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $(
                        table_names::$name => Ok(Self::$name),
                    )*
                    s => Err(format!("unknown table: {s:?}")),
                }
            }
        }

        // Need constants to match on in the `FromStr` implementation.
        #[allow(non_upper_case_globals)]
        mod table_names {
            $(
                pub(super) const $name: &'static str = stringify!($name);
            )*
        }
    };
}

tables! {
    /// Stores the header hashes belonging to the canonical chain.
    table CanonicalHeaders<Key = BlockNumber, Value = HeaderHash>;

    /// Stores the total difficulty from a block header.
    table HeaderTerminalDifficulties<Key = BlockNumber, Value = CompactU256>;

    /// Stores the block number corresponding to a header.
    table HeaderNumbers<Key = BlockHash, Value = BlockNumber>;

    /// Stores header bodies.
    table Headers<Key = BlockNumber, Value = Header>;

    /// Stores block indices that contains indexes of transaction and the count of them.
    ///
    /// More information about stored indices can be found in the [`StoredBlockBodyIndices`] struct.
    table BlockBodyIndices<Key = BlockNumber, Value = StoredBlockBodyIndices>;

    /// Stores the uncles/ommers of the block.
    table BlockOmmers<Key = BlockNumber, Value = StoredBlockOmmers>;

    /// Stores the block withdrawals.
    table BlockWithdrawals<Key = BlockNumber, Value = StoredBlockWithdrawals>;

    /// Canonical only Stores the transaction body for canonical transactions.
    table Transactions<Key = TxNumber, Value = TransactionSignedNoHash>;

    /// Stores the mapping of the transaction hash to the transaction number.
    table TransactionHashNumbers<Key = TxHash, Value = TxNumber>;

    /// Stores the mapping of transaction number to the blocks number.
    ///
    /// The key is the highest transaction ID in the block.
    table TransactionBlocks<Key = TxNumber, Value = BlockNumber>;

    /// Canonical only Stores transaction receipts.
    table Receipts<Key = TxNumber, Value = Receipt>;

    /// Stores all smart contract bytecodes.
    /// There will be multiple accounts that have same bytecode
    /// So we would need to introduce reference counter.
    /// This will be small optimization on state.
    table Bytecodes<Key = B256, Value = Bytecode>;

    /// Stores the current state of an [`Account`].
    table PlainAccountState<Key = Address, Value = Account>;

    /// Stores the current value of a storage key.
    table PlainStorageState<Key = Address, Value = StorageEntry, SubKey = B256>;

    /// Stores pointers to block changeset with changes for each account key.
    ///
    /// Last shard key of the storage will contain `u64::MAX` `BlockNumber`,
    /// this would allows us small optimization on db access when change is in plain state.
    ///
    /// Imagine having shards as:
    /// * `Address | 100`
    /// * `Address | u64::MAX`
    ///
    /// What we need to find is number that is one greater than N. Db `seek` function allows us to fetch
    /// the shard that equal or more than asked. For example:
    /// * For N=50 we would get first shard.
    /// * for N=150 we would get second shard.
    /// * If max block number is 200 and we ask for N=250 we would fetch last shard and
    ///     know that needed entry is in `AccountPlainState`.
    /// * If there were no shard we would get `None` entry or entry of different storage key.
    ///
    /// Code example can be found in `reth_provider::HistoricalStateProviderRef`
    table AccountsHistory<Key = ShardedKey<Address>, Value = BlockNumberList>;

    /// Stores pointers to block number changeset with changes for each storage key.
    ///
    /// Last shard key of the storage will contain `u64::MAX` `BlockNumber`,
    /// this would allows us small optimization on db access when change is in plain state.
    ///
    /// Imagine having shards as:
    /// * `Address | StorageKey | 100`
    /// * `Address | StorageKey | u64::MAX`
    ///
    /// What we need to find is number that is one greater than N. Db `seek` function allows us to fetch
    /// the shard that equal or more than asked. For example:
    /// * For N=50 we would get first shard.
    /// * for N=150 we would get second shard.
    /// * If max block number is 200 and we ask for N=250 we would fetch last shard and
    ///     know that needed entry is in `StoragePlainState`.
    /// * If there were no shard we would get `None` entry or entry of different storage key.
    ///
    /// Code example can be found in `reth_provider::HistoricalStateProviderRef`
    table StoragesHistory<Key = StorageShardedKey, Value = BlockNumberList>;

    /// Stores the state of an account before a certain transaction changed it.
    /// Change on state can be: account is created, selfdestructed, touched while empty
    /// or changed balance,nonce.
    table AccountChangeSets<Key = BlockNumber, Value = AccountBeforeTx, SubKey = Address>;

    /// Stores the state of a storage key before a certain transaction changed it.
    /// If [`StorageEntry::value`] is zero, this means storage was not existing
    /// and needs to be removed.
    table StorageChangeSets<Key = BlockNumberAddress, Value = StorageEntry, SubKey = B256>;

    /// Stores the current state of an [`Account`] indexed with `keccak256Address`
    /// This table is in preparation for merkelization and calculation of state root.
    /// We are saving whole account data as it is needed for partial update when
    /// part of storage is changed. Benefit for merkelization is that hashed addresses are sorted.
    table HashedAccounts<Key = B256, Value = Account>;

    /// Stores the current storage values indexed with `keccak256Address` and
    /// hash of storage key `keccak256key`.
    /// This table is in preparation for merkelization and calculation of state root.
    /// Benefit for merklization is that hashed addresses/keys are sorted.
    table HashedStorages<Key = B256, Value = StorageEntry, SubKey = B256>;

    /// Stores the current state's Merkle Patricia Tree.
    table AccountsTrie<Key = StoredNibbles, Value = StoredBranchNode>;

    /// From HashedAddress => NibblesSubKey => Intermediate value
    table StoragesTrie<Key = B256, Value = StorageTrieEntry, SubKey = StoredNibblesSubKey>;

    /// Stores the transaction sender for each canonical transaction.
    /// It is needed to speed up execution stage and allows fetching signer without doing
    /// transaction signed recovery
    table TransactionSenders<Key = TxNumber, Value = Address>;

    /// Stores the highest synced block number and stage-specific checkpoint of each stage.
    table StageCheckpoints<Key = StageId, Value = StageCheckpoint>;

    /// Stores arbitrary data to keep track of a stage first-sync progress.
    table StageCheckpointProgresses<Key = StageId, Value = Vec<u8>>;

    /// Stores the highest pruned block number and prune mode of each prune segment.
    table PruneCheckpoints<Key = PruneSegment, Value = PruneCheckpoint>;

    /// Stores the history of client versions that have accessed the database with write privileges by unix timestamp in seconds.
    table VersionHistory<Key = u64, Value = ClientVersion>;
}

// Alias types.

/// List with transaction numbers.
pub type BlockNumberList = IntegerList;

/// Encoded stage id.
pub type StageId = String;

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn parse_table_from_str() {
        for table in Tables::ALL {
            assert_eq!(format!("{table:?}"), table.name());
            assert_eq!(table.to_string(), table.name());
            assert_eq!(Tables::from_str(table.name()).unwrap(), *table);
        }
    }
}
