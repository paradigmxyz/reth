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

pub mod codecs;
pub mod models;
mod raw;
pub(crate) mod utils;

use crate::abstraction::table::Table;
pub use raw::{RawDupSort, RawKey, RawTable, RawValue, TableRawRow};
use std::{fmt::Display, str::FromStr};

/// Declaration of all Database tables.
use crate::{
    table::DupSort,
    tables::{
        codecs::CompactU256,
        models::{
            accounts::{AccountBeforeTx, BlockNumberAddress},
            blocks::{HeaderHash, StoredBlockOmmers},
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

/// Enum for the types of tables present in libmdbx.
#[derive(Debug, PartialEq, Copy, Clone)]
pub enum TableType {
    /// key value table
    Table,
    /// Duplicate key value table
    DupSort,
}

/// Number of tables that should be present inside database.
pub const NUM_TABLES: usize = 26;

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
/// use std::str::FromStr;
///
/// let headers = Tables::from_str("Headers").unwrap();
/// let transactions = Tables::from_str("Transactions").unwrap();
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
/// let _ = headers.view(&viewer);
/// let _ = transactions.view(&viewer);
/// ```
pub trait TableViewer<R> {
    /// The error type returned by the viewer.
    type Error;

    /// Operate on the table in a generic way.
    fn view<T: Table>(&self) -> Result<R, Self::Error>;

    /// Operate on the dupsort table in a generic way.
    /// By default, the `view` function is invoked unless overridden.
    fn view_dupsort<T: DupSort>(&self) -> Result<R, Self::Error> {
        self.view::<T>()
    }
}

macro_rules! tables {
    ([
        (TableType::Table, [$($table:ident),*]),
        (TableType::DupSort, [$($dupsort:ident),*])
    ]) => {
        #[derive(Debug, PartialEq, Copy, Clone)]
        /// Default tables that should be present inside database.
        pub enum Tables {
            $(
                #[doc = concat!("Represents a ", stringify!($table), " table")]
                $table,
            )*
            $(
                #[doc = concat!("Represents a ", stringify!($dupsort), " dupsort table")]
                $dupsort,
            )*
        }

        impl Tables {
            /// Array of all tables in database
            pub const ALL: [Tables; NUM_TABLES] = [$(Tables::$table,)* $(Tables::$dupsort,)*];

            /// The name of the given table in database
            pub const fn name(&self) -> &str {
                match self {
                    $(Tables::$table => {
                        $table::NAME
                    },)*
                    $(Tables::$dupsort => {
                        $dupsort::NAME
                    },)*
                }
            }

            /// The type of the given table in database
            pub const fn table_type(&self) -> TableType {
                match self {
                    $(Tables::$table => {
                        TableType::Table
                    },)*
                    $(Tables::$dupsort => {
                        TableType::DupSort
                    },)*
                }
            }

            /// Allows to operate on specific table type
            pub fn view<T, R>(&self, visitor: &T) -> Result<R, T::Error>
            where
                T: TableViewer<R>,
            {
                match self {
                    $(Tables::$table => {
                        visitor.view::<$table>()
                    },)*
                    $(Tables::$dupsort => {
                        visitor.view_dupsort::<$dupsort>()
                    },)*
                }
            }
        }

        impl Display for Tables {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.name())
            }
        }

        impl FromStr for Tables {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($table::NAME => {
                        Ok(Tables::$table)
                    },)*
                    $($dupsort::NAME => {
                        Ok(Tables::$dupsort)
                    },)*
                    _ => {
                        Err("Unknown table".to_string())
                    }
                }
            }
        }
    };
}

tables!([
    (
        TableType::Table,
        [
            CanonicalHeaders,
            HeaderTD,
            HeaderNumbers,
            Headers,
            BlockBodyIndices,
            BlockOmmers,
            BlockWithdrawals,
            TransactionBlock,
            Transactions,
            TxHashNumber,
            Receipts,
            PlainAccountState,
            Bytecodes,
            AccountHistory,
            StorageHistory,
            HashedAccount,
            AccountsTrie,
            TxSenders,
            SyncStage,
            SyncStageProgress,
            PruneCheckpoints
        ]
    ),
    (
        TableType::DupSort,
        [PlainStorageState, AccountChangeSet, StorageChangeSet, HashedStorage, StoragesTrie]
    )
]);

/// Macro to declare key value table.
#[macro_export]
macro_rules! table {
    ($(#[$docs:meta])+ ( $table_name:ident ) $key:ty | $value:ty) => {
        $(#[$docs])+
        ///
        #[doc = concat!("Takes [`", stringify!($key), "`] as a key and returns [`", stringify!($value), "`].")]
        #[derive(Clone, Copy, Debug, Default)]
        pub struct $table_name;

        impl $crate::table::Table for $table_name {
            const NAME: &'static str = stringify!($table_name);
            type Key = $key;
            type Value = $value;
        }

        impl std::fmt::Display for $table_name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", stringify!($table_name))
            }
        }
    };
}

#[macro_export]
/// Macro to declare duplicate key value table.
macro_rules! dupsort {
    ($(#[$docs:meta])+ ( $table_name:ident ) $key:ty | [$subkey:ty] $value:ty) => {
        table!(
            $(#[$docs])+
            ///
            #[doc = concat!("`DUPSORT` table with subkey being: [`", stringify!($subkey), "`]")]
            ( $table_name ) $key | $value
        );
        impl DupSort for $table_name {
            type SubKey = $subkey;
        }
    };
}

//
//  TABLE DEFINITIONS
//

table!(
    /// Stores the header hashes belonging to the canonical chain.
    ( CanonicalHeaders ) BlockNumber | HeaderHash
);

table!(
    /// Stores the total difficulty from a block header.
    ( HeaderTD ) BlockNumber | CompactU256
);

table!(
    /// Stores the block number corresponding to a header.
    ( HeaderNumbers ) BlockHash | BlockNumber
);

table!(
    /// Stores header bodies.
    ( Headers ) BlockNumber | Header
);

table!(
    /// Stores block indices that contains indexes of transaction and the count of them.
    ///
    /// More information about stored indices can be found in the [`StoredBlockBodyIndices`] struct.
    ( BlockBodyIndices ) BlockNumber | StoredBlockBodyIndices
);

table!(
    /// Stores the uncles/ommers of the block.
    ( BlockOmmers ) BlockNumber | StoredBlockOmmers
);

table!(
    /// Stores the block withdrawals.
    ( BlockWithdrawals ) BlockNumber | StoredBlockWithdrawals
);

table!(
    /// (Canonical only) Stores the transaction body for canonical transactions.
    ( Transactions ) TxNumber | TransactionSignedNoHash
);

table!(
    /// Stores the mapping of the transaction hash to the transaction number.
    ( TxHashNumber ) TxHash | TxNumber
);

table!(
    /// Stores the mapping of transaction number to the blocks number.
    ///
    /// The key is the highest transaction ID in the block.
    ( TransactionBlock ) TxNumber | BlockNumber
);

table!(
    /// (Canonical only) Stores transaction receipts.
    ( Receipts ) TxNumber | Receipt
);

table!(
    /// Stores all smart contract bytecodes.
    /// There will be multiple accounts that have same bytecode
    /// So we would need to introduce reference counter.
    /// This will be small optimization on state.
    ( Bytecodes ) B256 | Bytecode
);

table!(
    /// Stores the current state of an [`Account`].
    ( PlainAccountState ) Address | Account
);

dupsort!(
    /// Stores the current value of a storage key.
    ( PlainStorageState ) Address | [B256] StorageEntry
);

table!(
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
    ( AccountHistory ) ShardedKey<Address> | BlockNumberList
);

table!(
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
    ( StorageHistory ) StorageShardedKey | BlockNumberList
);

dupsort!(
    /// Stores the state of an account before a certain transaction changed it.
    /// Change on state can be: account is created, selfdestructed, touched while empty
    /// or changed (balance,nonce).
    ( AccountChangeSet ) BlockNumber | [Address] AccountBeforeTx
);

dupsort!(
    /// Stores the state of a storage key before a certain transaction changed it.
    /// If [`StorageEntry::value`] is zero, this means storage was not existing
    /// and needs to be removed.
    ( StorageChangeSet ) BlockNumberAddress | [B256] StorageEntry
);

table!(
    /// Stores the current state of an [`Account`] indexed with `keccak256(Address)`
    /// This table is in preparation for merkelization and calculation of state root.
    /// We are saving whole account data as it is needed for partial update when
    /// part of storage is changed. Benefit for merkelization is that hashed addresses are sorted.
    ( HashedAccount ) B256 | Account
);

dupsort!(
    /// Stores the current storage values indexed with `keccak256(Address)` and
    /// hash of storage key `keccak256(key)`.
    /// This table is in preparation for merkelization and calculation of state root.
    /// Benefit for merklization is that hashed addresses/keys are sorted.
    ( HashedStorage ) B256 | [B256] StorageEntry
);

table!(
    /// Stores the current state's Merkle Patricia Tree.
    ( AccountsTrie ) StoredNibbles | StoredBranchNode
);

dupsort!(
    /// From HashedAddress => NibblesSubKey => Intermediate value
    ( StoragesTrie ) B256 | [StoredNibblesSubKey] StorageTrieEntry
);

table!(
    /// Stores the transaction sender for each canonical transaction.
    /// It is needed to speed up execution stage and allows fetching signer without doing
    /// transaction signed recovery
    ( TxSenders ) TxNumber | Address
);

table!(
    /// Stores the highest synced block number and stage-specific checkpoint of each stage.
    ( SyncStage ) StageId | StageCheckpoint
);

table!(
    /// Stores arbitrary data to keep track of a stage first-sync progress.
    ( SyncStageProgress ) StageId | Vec<u8>
);

table!(
    /// Stores the highest pruned block number and prune mode of each prune segment.
    ( PruneCheckpoints ) PruneSegment | PruneCheckpoint
);

/// Alias Types

/// List with transaction numbers.
pub type BlockNumberList = IntegerList;
/// Encoded stage id.
pub type StageId = String;

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    const TABLES: [(TableType, &str); NUM_TABLES] = [
        (TableType::Table, CanonicalHeaders::NAME),
        (TableType::Table, HeaderTD::NAME),
        (TableType::Table, HeaderNumbers::NAME),
        (TableType::Table, Headers::NAME),
        (TableType::Table, BlockBodyIndices::NAME),
        (TableType::Table, BlockOmmers::NAME),
        (TableType::Table, BlockWithdrawals::NAME),
        (TableType::Table, TransactionBlock::NAME),
        (TableType::Table, Transactions::NAME),
        (TableType::Table, TxHashNumber::NAME),
        (TableType::Table, Receipts::NAME),
        (TableType::Table, PlainAccountState::NAME),
        (TableType::Table, Bytecodes::NAME),
        (TableType::Table, AccountHistory::NAME),
        (TableType::Table, StorageHistory::NAME),
        (TableType::Table, HashedAccount::NAME),
        (TableType::Table, AccountsTrie::NAME),
        (TableType::Table, TxSenders::NAME),
        (TableType::Table, SyncStage::NAME),
        (TableType::Table, SyncStageProgress::NAME),
        (TableType::Table, PruneCheckpoints::NAME),
        (TableType::DupSort, PlainStorageState::NAME),
        (TableType::DupSort, AccountChangeSet::NAME),
        (TableType::DupSort, StorageChangeSet::NAME),
        (TableType::DupSort, HashedStorage::NAME),
        (TableType::DupSort, StoragesTrie::NAME),
    ];

    #[test]
    fn parse_table_from_str() {
        for (table_index, &(table_type, table_name)) in TABLES.iter().enumerate() {
            let table = Tables::from_str(table_name).unwrap();

            assert_eq!(table as usize, table_index);
            assert_eq!(table.table_type(), table_type);
            assert_eq!(table.name(), table_name);
        }
    }
}
