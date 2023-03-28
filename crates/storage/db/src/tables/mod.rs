//! Tables and data models.
//!
//! # Overview
//!
//! This module defines the tables in reth, as well as some table-related abstractions:
//!
//! - [`codecs`] integrates different codecs into [`Encode`] and [`Decode`]
//! - [`models`] defines the values written to tables
//!
//! # Database Tour
//!
//! TODO(onbjerg): Find appropriate format for this...

pub mod codecs;
pub mod models;
pub(crate) mod utils;

/// Declaration of all Database tables.
use crate::{
    table::DupSort,
    tables::{
        codecs::CompactU256,
        models::{
            accounts::{AccountBeforeTx, TransitionIdAddress},
            blocks::{HeaderHash, StoredBlockOmmers},
            storage_sharded_key::StorageShardedKey,
            ShardedKey, StoredBlockBody, StoredBlockWithdrawals,
        },
    },
};
use reth_primitives::{
    Account, Address, BlockHash, BlockNumber, Bytecode, Header, IntegerList, Receipt, StorageEntry,
    StorageTrieEntry, TransactionSigned, TransitionId, TxHash, TxNumber, H256,
};

/// Enum for the types of tables present in libmdbx.
#[derive(Debug)]
pub enum TableType {
    /// key value table
    Table,
    /// Duplicate key value table
    DupSort,
}

/// Default tables that should be present inside database.
pub const TABLES: [(TableType, &str); 27] = [
    (TableType::Table, CanonicalHeaders::const_name()),
    (TableType::Table, HeaderTD::const_name()),
    (TableType::Table, HeaderNumbers::const_name()),
    (TableType::Table, Headers::const_name()),
    (TableType::Table, BlockBodies::const_name()),
    (TableType::Table, BlockOmmers::const_name()),
    (TableType::Table, BlockWithdrawals::const_name()),
    (TableType::Table, TransactionBlock::const_name()),
    (TableType::Table, Transactions::const_name()),
    (TableType::Table, TxHashNumber::const_name()),
    (TableType::Table, Receipts::const_name()),
    (TableType::Table, PlainAccountState::const_name()),
    (TableType::DupSort, PlainStorageState::const_name()),
    (TableType::Table, Bytecodes::const_name()),
    (TableType::Table, BlockTransitionIndex::const_name()),
    (TableType::Table, TxTransitionIndex::const_name()),
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
];

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
    /// Stores the block number corresponding to an header.
    ( HeaderNumbers ) BlockHash | BlockNumber
);

table!(
    /// Stores header bodies.
    ( Headers ) BlockNumber | Header
);

table!(
    /// Stores block bodies.
    ( BlockBodies ) BlockNumber | StoredBlockBody
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
    (  Transactions ) TxNumber | TransactionSigned
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
    ( Bytecodes ) H256 | Bytecode
);

table!(
    /// Stores the mapping of block number to state transition id.
    /// The block transition marks the final state at the end of the block.
    /// Increment the transition if the block contains an addition block reward.
    /// If the block does not have a reward and transaction, the transition will be the same as the
    /// transition at the last transaction of this block.
    ( BlockTransitionIndex ) BlockNumber | TransitionId
);

table!(
    /// Stores the mapping of transaction number to state transition id.
    ( TxTransitionIndex ) TxNumber | TransitionId
);

table!(
    /// Stores the current state of an [`Account`].
    ( PlainAccountState ) Address | Account
);

dupsort!(
    /// Stores the current value of a storage key.
    ( PlainStorageState ) Address | [H256] StorageEntry
);

table!(
    /// Stores pointers to transition changeset with changes for each account key.
    ///
    /// Last shard key of the storage will contains `u64::MAX` `TransitionId`,
    /// this would allows us small optimization on db access when change is in plain state.
    ///
    /// Imagine having shards as:
    /// * `Address | 100`
    /// * `Address | u64::MAX`
    ///
    /// What we need to find is id that is one greater than N. Db `seek` function allows us to fetch
    /// the shard that equal or more than asked. For example:
    /// * For N=50 we would get first shard.
    /// * for N=150 we would get second shard.
    /// * If max transition id is 200 and we ask for N=250 we would fetch last shard and
    ///     know that needed entry is in `AccountPlainState`.
    /// * If there were no shard we would get `None` entry or entry of different storage key.
    ///
    /// Code example can be found in `reth_provider::HistoricalStateProviderRef`
    ( AccountHistory ) ShardedKey<Address> | TransitionList
);

table!(
    /// Stores pointers to transition changeset with changes for each storage key.
    ///
    /// Last shard key of the storage will contains `u64::MAX` `TransitionId`,
    /// this would allows us small optimization on db access when change is in plain state.
    ///
    /// Imagine having shards as:
    /// * `Address | StorageKey | 100`
    /// * `Address | StorageKey | u64::MAX`
    ///
    /// What we need to find is id that is one greater than N. Db `seek` function allows us to fetch
    /// the shard that equal or more than asked. For example:
    /// * For N=50 we would get first shard.
    /// * for N=150 we would get second shard.
    /// * If max transition id is 200 and we ask for N=250 we would fetch last shard and
    ///     know that needed entry is in `StoragePlainState`.
    /// * If there were no shard we would get `None` entry or entry of different storage key.
    ///
    /// Code example can be found in `reth_provider::HistoricalStateProviderRef`
    ( StorageHistory ) StorageShardedKey | TransitionList
);

dupsort!(
    /// Stores the state of an account before a certain transaction changed it.
    /// Change on state can be: account is created, selfdestructed, touched while empty
    /// or changed (balance,nonce).
    ( AccountChangeSet ) TransitionId | [Address] AccountBeforeTx
);

dupsort!(
    /// Stores the state of a storage key before a certain transaction changed it.
    /// If [`StorageEntry::value`] is zero, this means storage was not existing
    /// and needs to be removed.
    ( StorageChangeSet ) TransitionIdAddress | [H256] StorageEntry
);

table!(
    /// Stores the current state of an [`Account`] indexed with `keccak256(Address)`
    /// This table is in preparation for merkelization and calculation of state root.
    /// We are saving whole account data as it is needed for partial update when
    /// part of storage is changed. Benefit for merkelization is that hashed addresses are sorted.
    ( HashedAccount ) H256 | Account
);

dupsort!(
    /// Stores the current storage values indexed with `keccak256(Address)` and
    /// hash of storage key `keccak256(key)`.
    /// This table is in preparation for merkelization and calculation of state root.
    /// Benefit for merklization is that hashed addresses/keys are sorted.
    ( HashedStorage ) H256 | [H256] StorageEntry
);

table!(
    /// Stores the current state's Merkle Patricia Tree.
    ( AccountsTrie ) H256 | Vec<u8>
);

dupsort!(
    /// Stores the Merkle Patricia Trees of each [`Account`]'s storage.
    ( StoragesTrie ) H256 | [H256] StorageTrieEntry
);

table!(
    /// Stores the transaction sender for each transaction.
    /// It is needed to speed up execution stage and allows fetching signer without doing
    /// transaction signed recovery
    ( TxSenders ) TxNumber | Address
);

table!(
    /// Stores the highest synced block number of each stage.
    ( SyncStage ) StageId | BlockNumber
);

table!(
    /// Stores arbitrary data to keep track of a stage first-sync progress.
    ( SyncStageProgress ) StageId | Vec<u8>
);

///
/// Alias Types

/// List with transaction numbers.
pub type TransitionList = IntegerList;
/// Encoded stage id.
pub type StageId = String;

//
// TODO: Temporary types, until they're properly defined alongside with the Encode and Decode Trait
//
