//! Util functions for revm related ops

use alloy_primitives::{hex, Bytes};
use alloy_sol_types::{ContractError, GenericRevertReason};
use revm::{
    primitives::{SpecId, KECCAK_EMPTY},
    DatabaseRef,
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

/// Loads the code for the given account from the account itself or the database
///
/// Returns None if the code hash is the KECCAK_EMPTY hash
#[inline]
pub(crate) fn load_account_code<DB: DatabaseRef>(
    db: DB,
    db_acc: &revm::primitives::AccountInfo,
) -> Option<Bytes> {
    db_acc
        .code
        .as_ref()
        .map(|code| code.original_bytes())
        .or_else(|| {
            if db_acc.code_hash == KECCAK_EMPTY {
                None
            } else {
                db.code_by_hash_ref(db_acc.code_hash).ok().map(|code| code.original_bytes())
            }
        })
        .map(Into::into)
}

/// Returns a non empty revert reason if the output is a revert/error.
#[inline]
pub(crate) fn maybe_revert_reason(output: &[u8]) -> Option<String> {
    let reason = match GenericRevertReason::decode(output)? {
        GenericRevertReason::ContractError(err) => {
            match err {
                ContractError::Revert(revert) => {
                    // return the raw revert reason and don't use the revert's display message
                    revert.reason
                }
                err => err.to_string(),
            }
        }
        GenericRevertReason::RawString(err) => err,
    };
    if reason.is_empty() {
        None
    } else {
        Some(reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_sol_types::{GenericContractError, SolInterface};

    #[test]
    fn decode_empty_revert() {
        let reason = GenericRevertReason::decode("".as_bytes()).map(|x| x.to_string());
        assert_eq!(reason, Some("".to_string()));
    }

    #[test]
    fn decode_revert_reason() {
        let err = GenericContractError::Revert("my revert".into());
        let encoded = err.abi_encode();
        let reason = maybe_revert_reason(&encoded).unwrap();
        assert_eq!(reason, "my revert");
    }
}
