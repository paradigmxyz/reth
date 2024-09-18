use serde::{Deserialize, Serialize};
use reth_primitives::{Address, Bytes, Receipt, U256};

/// Telos EVM Account Table Row
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelosAccountTableRow {
    /// Removed - if true, this row was removed from storage
    pub removed: bool,
    /// Address
    pub address: Address,
    /// Account
    pub account: String,
    /// Nonce
    pub nonce: u64,
    /// Code
    pub code: Bytes,
    /// Balance
    pub balance: U256
}

/// Telos EVM Account State Table Row
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelosAccountStateTableRow {
    /// Removed - if true, this row was removed from storage
    pub removed: bool,
    /// Address
    pub address: Address,
    /// Key
    pub key: U256,
    /// Value
    pub value: U256
}


/// Telos Engine API Extra Fields
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelosEngineAPIExtraFields {
    /// State Diffs for Account Table
    pub statediffs_account: Option<Vec<TelosAccountTableRow>>,
    /// State Diffs for Account State Table
    pub statediffs_accountstate: Option<Vec<TelosAccountStateTableRow>>,
    /// Revision changes in block
    pub revision_changes: Option<(u64,u64)>,
    /// Gas price changes in block
    pub gasprice_changes: Option<(u64,U256)>,
    /// New addresses using `create` action in block
    pub new_addresses_using_create: Option<Vec<(u64,U256)>>,
    /// New addresses using `openwallet` action in block
    pub new_addresses_using_openwallet: Option<Vec<(u64,U256)>>,
    /// Receipts produced by telos.evm contract
    pub receipts: Option<Vec<Receipt>>
}