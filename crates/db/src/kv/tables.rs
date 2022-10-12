//! Declaration of all MDBX tables.

use crate::{
    kv::blocks::{BlockNumber_BlockHash, HeaderHash, NumTransactions, NumTxesInBlock},
    utils::TableType,
};
use reth_primitives::{Address, BlockNumber, Header};

/// Default tables that should be present inside database.
pub const TABLES: [(TableType, &str); 17] = [
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
    (TableType::Table, PlainState::const_name()),
    (TableType::Table, AccountHistory::const_name()),
    (TableType::Table, StorageHistory::const_name()),
    (TableType::DupSort, AccountChangeSet::const_name()),
    (TableType::DupSort, StorageChangeSet::const_name()),
    (TableType::Table, TxSenders::const_name()),
    (TableType::Table, Config::const_name()),
];

#[macro_export]
/// Macro to declare all necessary tables.
macro_rules! table {
    ($name:ident => $key:ty => $value:ty => $seek:ty) => {
        /// $name MDBX table.
        #[derive(Clone, Copy, Debug, Default)]
        pub struct $name;

        impl $crate::kv::table::Table for $name {
            type Key = $key;
            type Value = $value;
            type SeekKey = $seek;

            /// Return $name as it is present inside the database.
            fn name(&self) -> &'static str {
                $name::const_name()
            }
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

//
//  TABLE DEFINITIONS
//

table!(CanonicalHeaders => BlockNumber => HeaderHash);
table!(HeaderTD => BlockNumber_BlockHash => RlpTotalDifficulty);
table!(HeaderNumbers => BlockNumber_BlockHash => BlockNumber);
table!(Headers => BlockNumber_BlockHash => Header);

table!(BlockBodies => BlockNumber_BlockHash => NumTxesInBlock);
table!(CumulativeTxCount => BlockNumber_BlockHash => NumTransactions); // TODO U256?

table!(NonCanonicalTransactions => BlockNumber_BlockHash_TxId => RlpTxBody);
table!(Transactions => TxId => RlpTxBody); // Canonical only
table!(Receipts => TxId => Receipt); // Canonical only
table!(Logs => TxId => Receipt); // Canonical only

table!(PlainState => PlainStateKey => Vec<u8>);

table!(AccountHistory => Address => TxIdList);
table!(StorageHistory => Address_StorageKey => TxIdList);

table!(AccountChangeSet => TxId => AccountBeforeTx);
table!(StorageChangeSet => TxId => StorageKeyBeforeTx);

table!(TxSenders => TxId => Address); // Is it necessary?
table!(Config => ConfigKey => ConfigValue);

//
// TODO: Temporary types, until they're properly defined alongside with the Encode and Decode Trait
//

type ConfigKey = Vec<u8>;
type ConfigValue = Vec<u8>;
#[allow(non_camel_case_types)]
type BlockNumber_BlockHash_TxId = Vec<u8>;
type RlpTotalDifficulty = Vec<u8>;
type RlpTxBody = Vec<u8>;
type Receipt = Vec<u8>;
type TxId = u64; // TODO check size
type PlainStateKey = Address; // TODO new type will have to account for address_incarna_skey as well
type TxIdList = Vec<u8>;
#[allow(non_camel_case_types)]
type Address_StorageKey = Vec<u8>;
type AccountBeforeTx = Vec<u8>;
type StorageKeyBeforeTx = Vec<u8>;
