use crate::eth::revm_utils::EvmOverrides;
use reth_rpc_types::{state::StateOverride, BlockOverrides, Bundle, EthCallResponse};

use alloy_rpc_types::{TransactionInput, TransactionRequest};

use serde_derive::{Deserialize, Serialize};

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockOverrideSim {
    pub number: Option<U256>,
    pub difficulty: Option<U256>,
    pub time: Option<U64>,
    pub random: Option<B256>,
    pub base_fee: Option<U256>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimBlock {
    pub blockOverrides: Vector<BlockOverridesSim>,
    pub stateOverride: StateOverride,
    pub calls: Vec<TransactionRequest>,
}
