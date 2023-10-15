//! Util functions for revm related ops

use reth_primitives::{hex, Address, B256};
use revm::{
    interpreter::CreateInputs,
    primitives::{CreateScheme, SpecId},
};

/// creates the memory data in 32byte chunks
/// see <https://github.com/ethereum/go-ethereum/blob/366d2169fbc0e0f803b68c042b77b6b480836dbc/eth/tracers/logger/logger.go#L450-L452>
#[inline]
pub(crate) fn convert_memory(data: &[u8]) -> Vec<String> {
    let mut memory = Vec::with_capacity((data.len() + 31) / 32);
    for idx in (0..data.len()).step_by(32) {
        let len = std::cmp::min(idx + 32, data.len());
        memory.push(hex::encode(&data[idx..len]));
    }
    memory
}

/// Get the gas used, accounting for refunds
#[inline]
pub(crate) fn gas_used(spec: SpecId, spent: u64, refunded: u64) -> u64 {
    let refund_quotient = if SpecId::enabled(spec, SpecId::LONDON) { 5 } else { 2 };
    spent - (refunded).min(spent / refund_quotient)
}

/// Get the address of a contract creation
#[inline]
pub(crate) fn get_create_address(call: &CreateInputs, nonce: u64) -> Address {
    match call.scheme {
        CreateScheme::Create => call.caller.create(nonce),
        CreateScheme::Create2 { salt } => {
            call.caller.create2_from_code(B256::from(salt), call.init_code.clone())
        }
    }
}
