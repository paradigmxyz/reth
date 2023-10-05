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
    trie::{BranchNodeCompact, StorageTrieEntry, StoredNibbles, StoredNibblesSubKey},
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
/// use reth_db::{ table::Table, TableViewer, Tables };
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
///         // operate on table in generic way
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
    /// type of error to return
    type Error;

    /// operate on table in generic way
    fn view<T: Table>(&self) -> Result<R, Self::Error>;
}

macro_rules! tables {
    ([$(($table:ident, $type:expr)),*]) => {
        #[derive(Debug, PartialEq, Copy, Clone)]
        /// Default tables that should be present inside database.
        pub enum Tables {
            $(
                #[doc = concat!("Represents a ", stringify!($table), " table")]
                $table,
            )*
        }

        impl Tables {
            /// Array of all tables in database
            pub const ALL: [Tables; NUM_TABLES] = [$(Tables::$table,)*];

            /// The name of the given table in database
            pub const fn name(&self) -> &str {
                match self {
                    $(Tables::$table => {
                        $table::NAME
                    },)*
                }
            }

            /// The type of the given table in database
            pub const fn table_type(&self) -> TableType {
                match self {
                    $(Tables::$table => {
                        $type
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
                        return Ok(Tables::$table)
                    },)*
                    _ => {
                        return Err("Unknown table".to_string())
                    }
                }
            }
        }
    };
}

tables!([
    (CanonicalHeaders, TableType::Table),
    (HeaderTD, TableType::Table),
    (HeaderNumbers, TableType::Table),
    (Headers, TableType::Table),
    (BlockBodyIndices, TableType::Table),
    (BlockOmmers, TableType::Table),
    (BlockWithdrawals, TableType::Table),
    (TransactionBlock, TableType::Table),
    (Transactions, TableType::Table),
    (TxHashNumber, TableType::Table),
    (Receipts, TableType::Table),
    (PlainAccountState, TableType::Table),
    (PlainStorageState, TableType::DupSort),
    (Bytecodes, TableType::Table),
    (AccountHistory, TableType::Table),
    (StorageHistory, TableType::Table),
    (AccountChangeSet, TableType::DupSort),
    (StorageChangeSet, TableType::DupSort),
    (HashedAccount, TableType::Table),
    (HashedStorage, TableType::DupSort),
    (AccountsTrie, TableType::Table),
    (StoragesTrie, TableType::DupSort),
    (TxSenders, TableType::Table),
    (SyncStage, TableType::Table),
    (SyncStageProgress, TableType::Table),
    (PruneCheckpoints, TableType::Table)
]);

#[macro_export]
/// Macro to declare key value table.
macro_rules! table {
    ($(#[$docs:meta])+ ( $table_name:ident ) $key:ty | $value:ty) => {
        $(#[$docs])+
        ///
        #[doc = concat!("Takes [`", stringify!($key), "`] as a key and returns [`", stringify!($value), "`]")]
        #[derive(Clone, Copy, Debug, Default)]
        pub struct $table_name;

        impl $crate::table::Table for $table_name {
            const NAME: &'static str = $table_name::const_name();
            type Key = $key;
            type Value = $value;
        }

        impl $table_name {
            #[doc=concat!("Return ", stringify!($table_name), " as it is present inside the database.")]
            pub const fn const_name() -> &'static str {
                stringify!($table_name)
            }
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
            #[doc = concat!("`DUPSORT` table with subkey being: [`", stringify!($subkey), "`].")]
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
    ( AccountsTrie ) StoredNibbles | BranchNodeCompact
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
    use std::str::FromStr;

    use crate::*;

    const TABLES: [(TableType, &str); NUM_TABLES] = [
        (TableType::Table, CanonicalHeaders::const_name()),
        (TableType::Table, HeaderTD::const_name()),
        (TableType::Table, HeaderNumbers::const_name()),
        (TableType::Table, Headers::const_name()),
        (TableType::Table, BlockBodyIndices::const_name()),
        (TableType::Table, BlockOmmers::const_name()),
        (TableType::Table, BlockWithdrawals::const_name()),
        (TableType::Table, TransactionBlock::const_name()),
        (TableType::Table, Transactions::const_name()),
        (TableType::Table, TxHashNumber::const_name()),
        (TableType::Table, Receipts::const_name()),
        (TableType::Table, PlainAccountState::const_name()),
        (TableType::DupSort, PlainStorageState::const_name()),
        (TableType::Table, Bytecodes::const_name()),
        (TableType::Table, AccountHistory::const_name()),
        (TableType::Table, StorageHistory::const_name()),
        (TableType::DupSort, AccountChangeSet::const_name()),
        (TableType::DupSort, StorageChangeSet::const_name()),
        (TableType::Table, HashedAccount::const_name()),
        (TableType::DupSort, HashedStorage::const_name()),
        (TableType::Table, AccountsTrie::const_name()),
        (TableType::DupSort, StoragesTrie::const_name()),
        (TableType::Table, TxSenders::const_name()),
        (TableType::Table, SyncStage::const_name()),
        (TableType::Table, SyncStageProgress::const_name()),
        (TableType::Table, PruneCheckpoints::const_name()),
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
