use crate::utils::TableType;
use reth_primitives::{Address, U256};

/// Default tables that should be present inside database.
pub const TABLES: [(TableType, &str); 17] = [
    (TableType::Table, CanonicalHeaders::name()),
    (TableType::Table, HeaderTD::name()),
    (TableType::Table, HeaderNumbers::name()),
    (TableType::Table, Headers::name()),
    (TableType::Table, BlockBodies::name()),
    (TableType::Table, CumulativeTxCount::name()),
    (TableType::Table, NonCanonicalTransactions::name()),
    (TableType::Table, Transactions::name()),
    (TableType::Table, Receipts::name()),
    (TableType::Table, Logs::name()),
    (TableType::Table, PlainState::name()),
    (TableType::Table, AccountHistory::name()),
    (TableType::Table, StorageHistory::name()),
    (TableType::DupSort, AccountChangeSet::name()),
    (TableType::DupSort, StorageChangeSet::name()),
    (TableType::Table, TxSenders::name()),
    (TableType::Table, Config::name()),
];

#[macro_export]
macro_rules! table {
    ($name:ident => $key:ty => $value:ty => $seek:ty) => {
        #[derive(Clone, Copy, Debug, Default)]
        pub struct $name;

        impl $crate::kv::table::Table for $name {
            type Key = $key;
            type Value = $value;
            type SeekKey = $seek;

            fn db_name(&self) -> &'static str  {
               stringify!($name)
            }
        }

        impl $name {
            pub const  fn name() -> &'static str {
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

table!(CanonicalHeaders => BNum => HeaderHash);
table!(HeaderTD => BNum_BHash => RlpTotalDifficulty);
table!(HeaderNumbers => BNum_BHash => BNum);
table!(Headers => BNum_BHash => RlpHeader);

table!(BlockBodies => BNum_BHash => NumTxesInBlock);
table!(CumulativeTxCount => BNum_BHash => u64); // TODO U256?

table!(NonCanonicalTransactions => BNum_BHash_TxId => RlpTxBody);
table!(Transactions => TxId => RlpTxBody); // Canonical only
table!(Receipts => TxId => Receipt); // Canonical only
table!(Logs => TxId => Receipt); // Canonical only

table!(PlainState => Address => Vec<u8>);

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
type BNum_BHash = Vec<u8>;
#[allow(non_camel_case_types)]
type BNum_BHash_TxId = Vec<u8>;
type RlpHeader = Vec<u8>;
type RlpTotalDifficulty = Vec<u8>;
type RlpTxBody = Vec<u8>;
type Receipt = Vec<u8>;
type NumTxesInBlock = u16; // TODO can it be u16
type BNum = u64; // TODO check size
type TxId = u64; // TODO check size
type HeaderHash = U256;
type PlainStateKey = Address; // TODO new type will have to account for address_incarna_skey as well
type AccountHistoryValue = Vec<u8>;
type TxIdList = Vec<u8>;
#[allow(non_camel_case_types)]
type Address_StorageKey = Vec<u8>;
type AccountBeforeTx = Vec<u8>;
type StorageKeyBeforeTx = Vec<u8>;
