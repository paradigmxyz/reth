#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256, bytes, Address, B256, Bytes, U256};
    use alloy_serde::WithOtherFields;
    use reth_arbitrum_primitives::ArbTypedTransaction;

    fn dummy_info() -> alloy_rpc_types_eth::TransactionInfo {
        alloy_rpc_types_eth::TransactionInfo {
            block_hash: Some(b256!("0x1111111111111111111111111111111111111111111111111111111111111111")),
            block_number: Some(0x1),
            transaction_index: Some(0),
            ..Default::default()
        }
    }

    fn signer() -> Address {
        address!("0xb8787d8f23e176a5d32135d746b69886e03313be")
    }

    #[test]
    fn maps_submit_retryable_fields() {
        let tx = ArbTransactionSigned::from(ArbTypedTransaction::SubmitRetryable(
            reth_arbitrum_primitives::tx::ArbSubmitRetryableTx {
                chain_id: U256::from(0x66eeeu64),
                request_id: b256!("0x01"),
                from: signer(),
                l1_base_fee: U256::from(0x5bd57bd9u64),
                deposit_value: U256::from_str_radix("23e3dbb7b88ab8", 16).unwrap(),
                gas_fee_cap: U256::from(0x3b9aca00u64),
                gas: 0x186a0,
                retry_to: Some(address!("0x3fab184622dc19b6109349b94811493bf2a45362")),
                retry_value: U256::from_str_radix("2386f26fc10000", 16).unwrap(),
                beneficiary: address!("0x11155ca9bbf7be58e27f3309e629c847996b43c8"),
                max_submission_fee: U256::from_str_radix("1f6377d4ab8", 16).unwrap(),
                fee_refund_addr: address!("0x11155ca9bbf7be58e27f3309e629c847996b43c8"),
                retry_data: Bytes::default(),
            },
        ));

        let resp: WithOtherFields<EthTransaction<ArbTransactionSigned>> =
            arb_tx_with_other_fields(&tx, signer(), dummy_info());

        use alloy_serde::OtherFields;
        let other = &resp.other;
        assert_eq!(other.get_deserialized::<B256>("requestId").unwrap().unwrap(), b256!("0x01"));
        assert_eq!(other.get_deserialized::<Address>("refundTo").unwrap().unwrap(), address!("0x11155ca9bbf7be58e27f3309e629c847996b43c8"));
        assert_eq!(other.get_deserialized::<U256>("l1BaseFee").unwrap().unwrap(), U256::from(0x5bd57bd9u64));
        assert_eq!(other.get_deserialized::<U256>("depositValue").unwrap().unwrap(), U256::from_str_radix("23e3dbb7b88ab8", 16).unwrap());
        assert_eq!(other.get_deserialized::<Address>("retryTo").unwrap().unwrap(), address!("0x3fab184622dc19b6109349b94811493bf2a45362"));
        assert_eq!(other.get_deserialized::<U256>("retryValue").unwrap().unwrap(), U256::from_str_radix("2386f26fc10000", 16).unwrap());
        let retry_data: Bytes = other.get_deserialized("retryData").unwrap().unwrap();
        assert_eq!(retry_data, Bytes::default());
        assert_eq!(other.get_deserialized::<Address>("beneficiary").unwrap().unwrap(), address!("0x11155ca9bbf7be58e27f3309e629c847996b43c8"));
        assert_eq!(other.get_deserialized::<U256>("maxSubmissionFee").unwrap().unwrap(), U256::from_str_radix("1f6377d4ab8", 16).unwrap());
    }

    #[test]
    fn maps_retry_fields() {
        let ticket = b256!("0x13cb79b086a427f3db7ebe6ec2bb90a806a3b0368ecee6020144f352e37dbdf6");
        let tx = ArbTransactionSigned::from(ArbTypedTransaction::Retry(
            reth_arbitrum_primitives::tx::ArbRetryTx {
                chain_id: U256::from(0x66eeeu64),
                nonce: 0,
                from: signer(),
                gas_fee_cap: U256::from(0x5f5e100u64),
                gas: 0x186a0,
                to: Some(address!("0x3fab184622dc19b6109349b94811493bf2a45362")),
                value: U256::from_str_radix("2386f26fc10000", 16).unwrap(),
                data: Bytes::default(),
                ticket_id: ticket,
                refund_to: address!("0x11155ca9bbf7be58e27f3309e629c847996b43c8"),
                max_refund: U256::from_str_radix("b0e85efeab8", 16).unwrap(),
                submission_fee_refund: U256::from_str_radix("1f6377d4ab8", 16).unwrap(),
            },
        ));

        let resp: WithOtherFields<EthTransaction<ArbTransactionSigned>> =
            arb_tx_with_other_fields(&tx, signer(), dummy_info());

        let other = &resp.other;
        assert_eq!(other.get_deserialized::<B256>("ticketId").unwrap().unwrap(), ticket);
        assert_eq!(other.get_deserialized::<U256>("maxRefund").unwrap().unwrap(), U256::from_str_radix("b0e85efeab8", 16).unwrap());
        assert_eq!(other.get_deserialized::<U256>("submissionFeeRefund").unwrap().unwrap(), U256::from_str_radix("1f6377d4ab8", 16).unwrap());
        assert_eq!(other.get_deserialized::<Address>("refundTo").unwrap().unwrap(), address!("0x11155ca9bbf7be58e27f3309e629c847996b43c8"));
    }
}

use alloy_primitives::{Address, Bytes, B256, U256};
use alloy_rpc_types_eth::{Transaction as EthTransaction, TransactionInfo};
use alloy_serde::{OtherFields, WithOtherFields};
use reth_arbitrum_primitives::{ArbTransactionSigned, ArbTypedTransaction};
use reth_primitives_traits::Recovered;
use reth_rpc_convert::transaction::FromConsensusTx;

pub fn arb_tx_with_other_fields(
    tx: &ArbTransactionSigned,
    signer: Address,
    tx_info: TransactionInfo,
) -> WithOtherFields<EthTransaction<ArbTransactionSigned>> {
    let inner = EthTransaction::from_transaction(Recovered::new_unchecked(tx.clone(), signer), tx_info);
    let mut out = WithOtherFields::new(inner);

    match &**tx {
        ArbTypedTransaction::SubmitRetryable(s) => {
            let _ = out.other.insert_value("requestId".to_string(), s.request_id);
            let _ = out.other.insert_value("refundTo".to_string(), s.fee_refund_addr);
            let _ = out.other.insert_value("l1BaseFee".to_string(), s.l1_base_fee);
            let _ = out.other.insert_value("depositValue".to_string(), s.deposit_value);
            if let Some(to) = s.retry_to {
                let _ = out.other.insert_value("retryTo".to_string(), to);
            }
            let _ = out.other.insert_value("retryValue".to_string(), s.retry_value);
            let _ = out.other.insert_value("retryData".to_string(), s.retry_data.clone());
            let _ = out.other.insert_value("beneficiary".to_string(), s.beneficiary);
            let _ = out.other.insert_value("maxSubmissionFee".to_string(), s.max_submission_fee);
        }
        ArbTypedTransaction::Retry(r) => {
            let _ = out.other.insert_value("ticketId".to_string(), r.ticket_id);
            let _ = out.other.insert_value("maxRefund".to_string(), r.max_refund);
            let _ = out.other.insert_value("submissionFeeRefund".to_string(), r.submission_fee_refund);
            let _ = out.other.insert_value("refundTo".to_string(), r.refund_to);
        }
        _ => {}
    }

    out
}
