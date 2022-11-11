//! Declaration of all Database tables.

use crate::db::{
    models::{
        accounts::{AccountBeforeTx, TxNumberAddress},
        blocks::{BlockNumHash, HeaderHash, NumTransactions, NumTxesInBlock},
        ShardedKey,
    },
    DupSort,
};
use reth_primitives::{
    Account, Address, BlockHash, BlockNumber, Header, IntegerList, Receipt, StorageEntry,
    TransactionSigned, TxNumber, H256,
};

/// Enum for the type of table present in libmdbx.
#[derive(Debug)]
pub enum TableType {
    /// key value table
    Table,
    /// Duplicate key value table
    DupSort,
}

/// Default tables that should be present inside database.
pub const TABLES: [(TableType, &str); 20] = [
    (TableType::Table, CanonicalHeaders::const_name()),
    (TableType::Table, HeaderTD::const_name()),
    (TableType::Table, HeaderNumbers::const_name()),
    (TableType::Table, Headers::const_name()),
    (TableType::Table, BlockBodies::const_name()),
    (TableType::Table, CumulativeTxCount::const_name()),
    (TableType::Table, NonCanonicalTransactions::const_name()),
    (TableType::Table, Transactions::const_name()),
    (TableType::Table, Receipts::const_name()),
    (TableType::Table, Logs::const_name()),
    (TableType::Table, PlainAccountState::const_name()),
    (TableType::DupSort, PlainStorageState::const_name()),
    (TableType::Table, Bytecodes::const_name()),
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
    (
    $(#[$docs:meta])+ $name:ident => $key:ty => $value:ty => $seek:ty) => {
        $(#[$docs])+
        ///
        #[doc = concat!("Takes [`", stringify!($key), "`] as a key and returns [`", stringify!($value), "`]")]
        #[derive(Clone, Copy, Debug, Default)]
        pub struct $name;

        impl $crate::db::table::Table for $name {
            const NAME: &'static str = $name::const_name();
            type Key = $key;
            type Value = $value;
        }

        impl $name {
            #[doc=concat!("Return ", stringify!($name), " as it is present inside the database.")]
            pub const fn const_name() -> &'static str {
                stringify!($name)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", stringify!($name))
            }
        }
    };
    ($(#[$docs:meta])+ $name:ident => $key:ty => $value:ty) => {
        table!(
            $(#[$docs])+
            $name => $key => $value => $key
        );
    };
}

macro_rules! dupsort {
    ($(#[$docs:meta])+ $name:ident => $key:ty => [$subkey:ty] $value:ty) => {
        table!(
            $(#[$docs])+
            ///
            #[doc = concat!("`DUPSORT` table with subkey being: [`", stringify!($subkey), "`].")]
            $name => $key => $value => $subkey
        );
        impl DupSort for $name {
            type SubKey = $subkey;
        }
    };
}

//
//  TABLE DEFINITIONS
//

table!(
    /// Stores the header hashes belonging to the canonical chain.
    CanonicalHeaders => BlockNumber => HeaderHash);

table!(
    /// Stores the total difficulty from a block header.
    HeaderTD => BlockNumHash => RlpTotalDifficulty);

table!(
    /// Stores the block number corresponding to an header.
    HeaderNumbers => BlockHash => BlockNumber);

table!(
    /// Stores header bodies.
    Headers => BlockNumHash => Header);

table!(
    /// Stores the number of transactions of a block.
    BlockBodies => BlockNumHash => NumTxesInBlock);

table!(
    /// Stores the maximum [`TxNumber`] from which this particular block starts.
    CumulativeTxCount => BlockNumHash => NumTransactions); // TODO U256?

table!(
    /// Stores the transaction body from non canonical transactions.
    NonCanonicalTransactions => BlockNumHashTxNumber => TransactionSigned);

table!(
    /// Stores the transaction body from canonical transactions. Canonical only
    Transactions => TxNumber => TransactionSigned);

table!(
    /// Stores transaction receipts. Canonical only
    Receipts => TxNumber => Receipt);

table!(
    /// Stores transaction logs. Canonical only
    Logs => TxNumber => Receipt);

table!(
    /// Stores the current state of an Account.
    PlainAccountState => Address => Account);

table!(
    /// Stores all smart contract bytecodes.
    Bytecodes => H256 => Bytecode);

dupsort!(
    /// Stores the current value of a storage key.
    PlainStorageState => Address => [H256] StorageEntry);

table!(
    /// Stores the transaction numbers that changed each account.
    /// 
    /// ```
    /// use reth_primitives::{Address, IntegerList};
    /// use reth_interfaces::db::{DbTx, DbTxMut, DbCursorRO, Database, models::ShardedKey, tables::AccountHistory};
    /// use reth_db::{kv::{EnvKind, Env, test_utils}, mdbx::WriteMap};
    /// use std::{str::FromStr,sync::Arc};
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
    AccountHistory => ShardedKey<Address> => TxNumberList);

table!(
    /// Stores the transaction numbers that changed each storage key.
    StorageHistory => AddressStorageKey => TxNumberList);

dupsort!(
    /// Stores state of an account before a certain transaction changed it.
    AccountChangeSet => TxNumber => [Address] AccountBeforeTx);

dupsort!(
    /// Stores state of a storage key before a certain transaction changed it.
    StorageChangeSet => TxNumberAddress => [H256] StorageEntry);

table!(
    /// Stores the transaction sender from each transaction.
    TxSenders => TxNumber => Address); // Is it necessary? if so, inverted index index so we dont repeat addresses?

table!(
    /// Config.
    Config => ConfigKey => ConfigValue);

table!(
    /// Stores the block number of each stage id.
    SyncStage => StageId => BlockNumber);

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
pub type RlpTotalDifficulty = Vec<u8>;
/// Temporary placeholder type for DB.
pub type AddressStorageKey = Vec<u8>;
/// Temporary placeholder type for DB.
pub type Bytecode = Vec<u8>;
