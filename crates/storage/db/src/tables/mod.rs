//! Table and data structures

pub mod codecs;
pub mod models;
pub mod utils;

/// Declaration of all Database tables.
use crate::{
    table::DupSort,
    tables::{
        codecs::CompactU256,
        models::{
            accounts::{AccountBeforeTx, TransitionIdAddress},
            blocks::{HeaderHash, StoredBlockOmmers},
            BlockNumHash, ShardedKey,
        },
    },
};
use reth_primitives::{
    Account, Address, BlockHash, BlockNumber, Header, IntegerList, Receipt, StorageEntry,
    TransactionSigned, TransitionId, TxHash, TxNumber, H256,
};

use self::models::StoredBlockBody;

/// Enum for the types of tables present in libmdbx.
#[derive(Debug)]
pub enum TableType {
    /// key value table
    Table,
    /// Duplicate key value table
    DupSort,
}

/// Default tables that should be present inside database.
pub const TABLES: [(TableType, &str); 23] = [
    (TableType::Table, CanonicalHeaders::const_name()),
    (TableType::Table, HeaderTD::const_name()),
    (TableType::Table, HeaderNumbers::const_name()),
    (TableType::Table, Headers::const_name()),
    (TableType::Table, BlockBodies::const_name()),
    (TableType::Table, BlockOmmers::const_name()),
    (TableType::Table, NonCanonicalTransactions::const_name()),
    (TableType::Table, Transactions::const_name()),
    (TableType::Table, TxHashNumber::const_name()),
    (TableType::Table, Receipts::const_name()),
    (TableType::Table, Logs::const_name()),
    (TableType::Table, PlainAccountState::const_name()),
    (TableType::DupSort, PlainStorageState::const_name()),
    (TableType::Table, Bytecodes::const_name()),
    (TableType::Table, BlockTransitionIndex::const_name()),
    (TableType::Table, TxTransitionIndex::const_name()),
    (TableType::Table, AccountHistory::const_name()),
    (TableType::Table, StorageHistory::const_name()),
    (TableType::DupSort, AccountChangeSet::const_name()),
    (TableType::DupSort, StorageChangeSet::const_name()),
    (TableType::Table, TxSenders::const_name()),
    (TableType::Table, Config::const_name()),
    (TableType::Table, SyncStage::const_name()),
];

#[macro_export]
/// Macro to declare all necessary tables.
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
                stringify!($table_name) }
        }

        impl std::fmt::Display for $table_name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", stringify!($table_name)) }
        }
    };
}

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
    ( HeaderTD ) BlockNumHash | CompactU256
);

table!(
    /// Stores the block number corresponding to an header.
    ( HeaderNumbers ) BlockHash | BlockNumber
);

table!(
    /// Stores header bodies.
    ( Headers ) BlockNumHash | Header
);

table!(
    /// Stores block bodies.
    ( BlockBodies ) BlockNumHash | StoredBlockBody
);

table!(
    /// Stores the uncles/ommers of the block.
    ( BlockOmmers ) BlockNumHash | StoredBlockOmmers
);

table!(
    /// Stores the transaction body from non canonical transactions.
    ( NonCanonicalTransactions ) BlockNumHashTxNumber | TransactionSigned
);

table!(
    /// (Canonical only) Stores the transaction body for canonical transactions.
    ( Transactions ) TxNumber | TransactionSigned
);

table!(
    /// Stores the mapping of the transaction hash to the transaction number.
    ( TxHashNumber ) TxHash | TxNumber
);

table!(
    /// (Canonical only) Stores transaction receipts.
    ( Receipts ) TxNumber | Receipt
);

table!(
    /// (Canonical only) Stores transaction logs.
    ( Logs ) TxNumber | Receipt
);

table!(
    /// Stores the current state of an [`Account`].
    ( PlainAccountState ) Address | Account
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
    ( BlockTransitionIndex ) BlockNumHash | TransitionId
);

table!(
    /// Stores the mapping of transaction number to state transition id.
    ( TxTransitionIndex ) TxNumber | TransitionId
);

dupsort!(
    /// Stores the current value of a storage key.
    ( PlainStorageState ) Address | [H256] StorageEntry
);

table!(
    /// Stores the transaction numbers that changed each account.
    ///
    /// ```
    /// use reth_primitives::{Address, IntegerList};
    /// use reth_db::{transaction::{DbTxMut,DbTx}, mdbx::{EnvKind, Env, test_utils,WriteMap}, cursor::DbCursorRO,database::Database, tables::{AccountHistory,models::ShardedKey}};
    /// use std::{str::FromStr,sync::Arc};
    ///
    ///
    /// fn main() {
    ///     let db: Arc<Env<WriteMap>> = test_utils::create_test_db(EnvKind::RW);
    ///     let account = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047").unwrap();
    ///
    ///     // Setup if each shard can only take 1 transaction.
    ///     for i in 1..3 {
    ///         let key = ShardedKey::new(account, i * 100);
    ///         let list: IntegerList = vec![i * 100u64].into();

    ///         db.update(|tx| tx.put::<AccountHistory>(key.clone(), list.clone()).expect("")).unwrap();
    ///     }
    ///
    ///     // Is there any transaction after number 150 that changed this account?
    ///     {
    ///         let tx = db.tx().expect("");
    ///         let mut cursor = tx.cursor::<AccountHistory>().unwrap();

    ///         // It will seek the one greater or equal to the query. Since we have `Address | 100`,
    ///         // `Address | 200` in the database and we're querying `Address | 150` it will return us
    ///         // `Address | 200`.
    ///         let mut walker = cursor.walk(ShardedKey::new(account, 150)).unwrap();
    ///         let (key, list) = walker
    ///             .next()
    ///             .expect("element should exist.")
    ///             .expect("should be able to retrieve it.");

    ///         assert_eq!(ShardedKey::new(account, 200), key);
    ///         let list200: IntegerList = vec![200u64].into();
    ///         assert_eq!(list200, list);
    ///         assert!(walker.next().is_none());
    ///     }
    /// }
    /// ```
    ///
    ( AccountHistory ) ShardedKey<Address> | TxNumberList
);

table!(
    /// Stores pointers to transactions that changed each storage key.
    ( StorageHistory ) AddressStorageKey | TxNumberList
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
    /// Stores the transaction sender for each transaction.
    /// It is needed to speed up execution stage and allows fetching signer without doing
    /// transaction signed recovery
    ( TxSenders ) TxNumber | Address
);

table!(
    /// Configuration values.
    ( Config ) ConfigKey | ConfigValue
);

table!(
    /// Stores the highest synced block number of each stage.
    ( SyncStage ) StageId | BlockNumber
);

///
/// Alias Types

/// List with transaction numbers.
pub type TxNumberList = IntegerList;
/// Encoded stage id.
pub type StageId = Vec<u8>;

//
// TODO: Temporary types, until they're properly defined alongside with the Encode and Decode Trait
//

/// Temporary placeholder type for DB.
pub type ConfigKey = Vec<u8>;
/// Temporary placeholder type for DB.
pub type ConfigValue = Vec<u8>;
/// Temporary placeholder type for DB.
pub type BlockNumHashTxNumber = Vec<u8>;
/// Temporary placeholder type for DB.
pub type AddressStorageKey = Vec<u8>;
/// Temporary placeholder type for DB.
pub type Bytecode = Vec<u8>;
