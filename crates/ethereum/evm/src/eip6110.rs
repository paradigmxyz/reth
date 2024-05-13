//! EIP-6110 deposit requests parsing
use alloy_consensus::Request;
use alloy_eips::eip6110::{DepositRequest, MAINNET_DEPOSIT_CONTRACT_ADDRESS};
use alloy_sol_types::{sol, SolEvent};
use reth_interfaces::executor::BlockValidationError;
use reth_primitives::Receipt;
use revm_primitives::Log;

pub(crate) fn parse_deposits_from_receipts(
    receipts: &[Receipt],
) -> Result<Vec<Request>, BlockValidationError> {
    let res = receipts
        .iter()
        .flat_map(|receipt| receipt.logs.iter())
        // No need to filter for topic because there's only one event and that's the Deposit event
        // in the deposit contract.
        .filter(|log| log.address == MAINNET_DEPOSIT_CONTRACT_ADDRESS)
        .map(|log| {
            let decoded_log = DepositEvent::decode_log(log, false)?;
            let deposit = parse_deposit_from_log(&decoded_log);
            Ok(Request::DepositRequest(deposit))
        })
        .collect::<Result<Vec<_>, _>>()
        // todo: this is ugly, we should clean it up
        .map_err(|err: alloy_sol_types::Error| {
            BlockValidationError::DepositRequestDecode(err.to_string())
        })?;
    Ok(res)
}

sol! {
    #[allow(missing_docs)]
    event DepositEvent(
        bytes pubkey,
        bytes withdrawal_credentials,
        bytes amount,
        bytes signature,
        bytes index
    );
}

fn parse_deposit_from_log(log: &Log<DepositEvent>) -> DepositRequest {
    // SAFETY: These `expect` https://github.com/ethereum/consensus-specs/blob/5f48840f4d768bf0e0a8156a3ed06ec333589007/solidity_deposit_contract/deposit_contract.sol#L107-L110
    // are safe because the `DepositEvent` is the only event in the deposit contract and the length
    // checks are done there.
    DepositRequest {
        pubkey: log
            .pubkey
            .as_ref()
            .try_into()
            .expect("pubkey length should be enforced in deposit contract"),
        withdrawal_credentials: log
            .withdrawal_credentials
            .as_ref()
            .try_into()
            .expect("withdrawal_credentials length should be enforced in deposit contract"),
        amount: u64::from_le_bytes(
            log.amount
                .as_ref()
                .try_into()
                .expect("amount length should be enforced in deposit contract"),
        ),
        signature: log
            .signature
            .as_ref()
            .try_into()
            .expect("signature length should be enforced in deposit contract"),
        index: u64::from_le_bytes(
            log.index
                .as_ref()
                .try_into()
                .expect("deposit index length should be enforced in deposit contract"),
        ),
    }
}
