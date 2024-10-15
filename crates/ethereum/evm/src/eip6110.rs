//! EIP-6110 deposit requests parsing
use alloc::{string::ToString, vec::Vec};
use alloy_eips::eip6110::{DepositRequest, MAINNET_DEPOSIT_CONTRACT_ADDRESS};
use alloy_primitives::Log;
use alloy_sol_types::{sol, SolEvent};
use reth_chainspec::ChainSpec;
use reth_evm::execute::BlockValidationError;
use reth_primitives::{Receipt, Request};

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

/// Parse [deposit contract](https://etherscan.io/address/0x00000000219ab540356cbb839cbe05303d7705fa)
/// (address is from the passed [`ChainSpec`]) deposits from receipts, and return them as a
/// [vector](Vec) of (requests)[Request].
pub fn parse_deposits_from_receipts<'a, I>(
    chain_spec: &ChainSpec,
    receipts: I,
) -> Result<Vec<Request>, BlockValidationError>
where
    I: IntoIterator<Item = &'a Receipt>,
{
    let deposit_contract_address = chain_spec
        .deposit_contract
        .as_ref()
        .map_or(MAINNET_DEPOSIT_CONTRACT_ADDRESS, |contract| contract.address);
    receipts
        .into_iter()
        .flat_map(|receipt| receipt.logs.iter())
        // No need to filter for topic because there's only one event and that's the Deposit event
        // in the deposit contract.
        .filter(|log| log.address == deposit_contract_address)
        .map(|log| {
            let decoded_log = DepositEvent::decode_log(log, false)?;
            let deposit = parse_deposit_from_log(&decoded_log);
            Ok(Request::DepositRequest(deposit))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err: alloy_sol_types::Error| {
            BlockValidationError::DepositRequestDecode(err.to_string())
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::MAINNET;
    use reth_primitives::TxType;

    #[test]
    fn test_parse_deposit_from_log() {
        let receipts = vec![
            // https://etherscan.io/tx/0xa5239d4c542063d29022545835815b78b09f571f2bf1c8427f4765d6f5abbce9
            #[allow(clippy::needless_update)] // side-effect of optimism fields
            Receipt {
                // these don't matter
                tx_type: TxType::Legacy,
                success: true,
                cumulative_gas_used: 0,
                logs: serde_json::from_str(
                    r#"[{"address":"0x00000000219ab540356cbb839cbe05303d7705fa","topics":["0x649bbc62d0e31342afea4e5cd82d4049e7e1ee912fc0889aa790803be39038c5"],"data":"0x00000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000000140000000000000000000000000000000000000000000000000000000000000018000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000030998c8086669bf65e24581cda47d8537966e9f5066fc6ffdcba910a1bfb91eae7a4873fcce166a1c4ea217e6b1afd396200000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002001000000000000000000000001c340fb72ed14d4eaa71f7633ee9e33b88d4f3900000000000000000000000000000000000000000000000000000000000000080040597307000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006098ddbffd700c1aac324cfdf0492ff289223661eb26718ce3651ba2469b22f480d56efab432ed91af05a006bde0c1ea68134e0acd8cacca0c13ad1f716db874b44abfcc966368019753174753bca3af2ea84bc569c46f76592a91e97f311eddec0000000000000000000000000000000000000000000000000000000000000008e474160000000000000000000000000000000000000000000000000000000000","blockHash":"0x8d1289c5a7e0965b1d1bb75cdc4c3f73dda82d4ebb94ff5b98d1389cebd53b56","blockNumber":"0x12f0d8d","transactionHash":"0xa5239d4c542063d29022545835815b78b09f571f2bf1c8427f4765d6f5abbce9","transactionIndex":"0xc4","logIndex":"0x18f","removed":false}]"#
                ).unwrap(),
                ..Default::default()
            },
            // https://etherscan.io/tx/0xd9734d4e3953bcaa939fd1c1d80950ee54aeecc02eef6ae8179f47f5b7103338
            #[allow(clippy::needless_update)] // side-effect of optimism fields
            Receipt {
                // these don't matter
                tx_type: TxType::Legacy,
                success: true,
                cumulative_gas_used: 0,
                logs: serde_json::from_str(
                    r#"[{"address":"0x00000000219ab540356cbb839cbe05303d7705fa","topics":["0x649bbc62d0e31342afea4e5cd82d4049e7e1ee912fc0889aa790803be39038c5"],"data":"0x00000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000000140000000000000000000000000000000000000000000000000000000000000018000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000030a1a2ba870a90e889aa594a0cc1c6feffb94c2d8f65646c937f1f456a315ef649533e25a4614d8f4f66ebdb06481b90af0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000200100000000000000000000000a0f04a231efbc29e1db7d086300ff550211c2f6000000000000000000000000000000000000000000000000000000000000000800405973070000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000060ad416d590e1a7f52baff770a12835b68904efad22cc9f8ba531e50cbbd26f32b9c7373cf6538a0577f501e4d3e3e63e208767bcccaae94e1e3720bfb734a286f9c017d17af46536545ccb7ca94d71f295e71f6d25bf978c09ada6f8d3f7ba0390000000000000000000000000000000000000000000000000000000000000008e374160000000000000000000000000000000000000000000000000000000000","blockHash":"0x8d1289c5a7e0965b1d1bb75cdc4c3f73dda82d4ebb94ff5b98d1389cebd53b56","blockNumber":"0x12f0d8d","transactionHash":"0xd9734d4e3953bcaa939fd1c1d80950ee54aeecc02eef6ae8179f47f5b7103338","transactionIndex":"0x7c","logIndex":"0xe2","removed":false}]"#,
                ).unwrap(),
                ..Default::default()
            },
        ];

        let requests = parse_deposits_from_receipts(&MAINNET, &receipts).unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].as_deposit_request().unwrap().amount, 32e9 as u64);
        assert_eq!(requests[1].as_deposit_request().unwrap().amount, 32e9 as u64);
    }
}
