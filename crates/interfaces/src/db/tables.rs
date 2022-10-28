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
    Account, Address, BlockNumber, Header, IntegerList, Receipt, StorageEntry, TxNumber, H256,
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
pub const TABLES: [(TableType, &str); 19] = [
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
            type SeekKey = $seek;
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
    ($(#[$outer:meta])+ $name:ident => $key:ty => [$subkey:ty] $value:ty) => {
        table!(
            $(#[$outer])+
            $name => $key => $value
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
    HeaderNumbers => BlockNumHash => BlockNumber);

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
    NonCanonicalTransactions => BlockNumHashTxNumber => RlpTxBody);

table!(
    /// Stores the transaction body from canonical transactions. Canonical only
    Transactions => TxNumber => RlpTxBody);

table!(
    /// Stores transaction receipts. Canonical only
    Receipts => TxNumber => Receipt);

table!(
    /// Stores transaction logs. Canonical only
    Logs => TxNumber => Receipt);

table!(
    /// Stores the current state of an Account.
    PlainAccountState => Address => Account);

dupsort!(
    /// Stores the current value of a storage key.
    PlainStorageState => Address => [H256] StorageEntry);

table!(
    /// Stores the transaction numbers that changed each account.
    AccountHistory => ShardedKey<Address> => TxNumberList);

table!(
    /// Stores the transaction numbers that changed each storage key.
    StorageHistory => Address_StorageKey => TxNumberList);

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

pub type TxNumberList = IntegerList;

//
// TODO: Temporary types, until they're properly defined alongside with the Encode and Decode Trait
//

pub type ConfigKey = Vec<u8>;
pub type ConfigValue = Vec<u8>;
#[allow(non_camel_case_types)]
pub type BlockNumHashTxNumber = Vec<u8>;
pub type RlpTotalDifficulty = Vec<u8>;
pub type RlpTxBody = Vec<u8>;
#[allow(non_camel_case_types)]
pub type Address_StorageKey = Vec<u8>;
pub type StageId = Vec<u8>;
