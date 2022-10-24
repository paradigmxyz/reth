//! Declaration of all Database tables.

use crate::db::{
    models::blocks::{BlockNumHash, HeaderHash, NumTransactions, NumTxesInBlock},
    DupSort,
};
use reth_primitives::{
    Account, Address, BlockNumber, Header, IntegerList, Receipt, StorageEntry, H256,
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
    ($name:ident => $key:ty => $value:ty => $seek:ty) => {
        /// $name MDBX table.
        #[derive(Clone, Copy, Debug, Default)]
        pub struct $name;

        impl $crate::db::table::Table for $name {
            const NAME: &'static str = $name::const_name();
            type Key = $key;
            type Value = $value;
            type SeekKey = $seek;

        }

        impl $name {
            /// Return $name as it is present inside the database.
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
    ($name:ident => $key:ty => $value:ty) => {
        table!($name => $key => $value => $key);
    };
}

macro_rules! dupsort {
    ($name:ident => $key:ty => [$subkey:ty] $value:ty) => {
        table!($name => $key => $value);
        impl DupSort for $name {
            type SubKey = $subkey;
        }
    };
}

//
//  TABLE DEFINITIONS
//

table!(CanonicalHeaders => BlockNumber => HeaderHash);
table!(HeaderTD => BlockNumHash => RlpTotalDifficulty);
table!(HeaderNumbers => BlockNumHash => BlockNumber);
table!(Headers => BlockNumHash => Header);

table!(BlockBodies => BlockNumHash => NumTxesInBlock);
table!(CumulativeTxCount => BlockNumHash => NumTransactions); // TODO U256?

table!(NonCanonicalTransactions => BlockNumHashTxNumber => RlpTxBody);
table!(Transactions => TxNumber => RlpTxBody); // Canonical only
table!(Receipts => TxNumber => Receipt); // Canonical only
table!(Logs => TxNumber => Receipt); // Canonical only

table!(PlainAccountState => Address => Account);
dupsort!(PlainStorageState => Address => [H256] StorageEntry);

table!(AccountHistory => Address => TxNumberList);
table!(StorageHistory => Address_StorageKey => TxNumberList);

table!(AccountChangeSet => TxNumber => AccountBeforeTx);
table!(StorageChangeSet => TxNumber => StorageKeyBeforeTx);

table!(TxSenders => TxNumber => Address); // Is it necessary?
table!(Config => ConfigKey => ConfigValue);

table!(SyncStage => StageId => BlockNumber);

///
/// Alias Types

type TxNumberList = IntegerList;
type TxNumber = u64;

//
// TODO: Temporary types, until they're properly defined alongside with the Encode and Decode Trait
//

type ConfigKey = Vec<u8>;
type ConfigValue = Vec<u8>;
#[allow(non_camel_case_types)]
type BlockNumHashTxNumber = Vec<u8>;
type RlpTotalDifficulty = Vec<u8>;
type RlpTxBody = Vec<u8>;
#[allow(non_camel_case_types)]
type Address_StorageKey = Vec<u8>;
type AccountBeforeTx = Vec<u8>;
type StorageKeyBeforeTx = Vec<u8>;
type StageId = Vec<u8>;
