use alloy_consensus::Transaction;

use alloy_primitives::{bytes, address, Address, B256, U256};
#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256, bytes, Address, B256, Bytes, U256, Signature};
    use alloy_serde::WithOtherFields;
    use reth_arbitrum_primitives::{ArbTypedTransaction, ArbTransactionSigned};

    fn dummy_info() -> alloy_rpc_types_eth::TransactionInfo {
        alloy_rpc_types_eth::TransactionInfo {
            hash: Some(B256::ZERO),
            index: Some(0),
            base_fee: None,
            block_hash: None,
            block_number: None,
        }
    }

    fn signer() -> Address {
        address!("0xb8787d8f23e176a5d32135d746b69886e03313be")
    }

    #[cfg(test)]
    mod no_encoded_2718_field_in_rpc_json {
        use super::*;
        use serde_json::to_string;

        fn signer() -> Address {
            address!("0xA4B05FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF")
        }

        fn dummy_info() -> alloy_rpc_types_eth::TransactionInfo {
            alloy_rpc_types_eth::TransactionInfo {
                hash: Some(B256::ZERO),
                index: Some(0),
                base_fee: None,
                block_hash: None,
                block_number: None,
            }
        }

        #[test]
        fn rpc_tx_json_has_no_transaction_encoded_2718() {
            use arb_alloy_consensus::tx::ArbInternalTx;
            use reth_arbitrum_primitives::{ArbTypedTransaction, ArbTransactionSigned};

            let sys = ArbInternalTx {
                chain_id: U256::from(0x66eeeu64),
                data: bytes!("6bf6a42d"),
            };
            let tx = ArbTransactionSigned::new_unhashed(ArbTypedTransaction::Internal(sys), Signature::new(U256::ZERO, U256::ZERO, false));

            let resp: alloy_serde::WithOtherFields<
                alloy_rpc_types_eth::Transaction<reth_arbitrum_primitives::ArbTransactionSigned>
            > = arb_tx_with_other_fields(&tx, signer(), dummy_info());

            let json = to_string(&resp).unwrap();
            assert!(!json.contains("transaction_encoded_2718"));
        }
    }
    #[test]
    fn internal_tx_has_zero_gas_and_gas_price_in_rpc() {
        use arb_alloy_consensus::tx::ArbInternalTx;
        use alloy_primitives::Signature;
        use serde_json::Value;
        let sys = ArbInternalTx {
            chain_id: U256::from(0x66eeeu64),
            data: bytes!("6bf6a42d"),
        };
        let tx = ArbTransactionSigned::new_unhashed(ArbTypedTransaction::Internal(sys), Signature::new(U256::ZERO, U256::ZERO, false));
        let resp: WithOtherFields<EthTransaction<ArbTransactionSigned>> =
            arb_tx_with_other_fields(&tx, signer(), dummy_info());
        let json = serde_json::to_value(&resp).unwrap();
        let obj = json.as_object().unwrap();
        assert_eq!(obj.get("gas").unwrap(), "0x0");
        assert_eq!(obj.get("gasPrice").unwrap(), "0x0");
    }


    #[test]
    fn maps_submit_retryable_fields() {
        use alloy_primitives::Signature;
        let tx = ArbTransactionSigned::new_unhashed(
            ArbTypedTransaction::SubmitRetryable(
                arb_alloy_consensus::tx::ArbSubmitRetryableTx {
                    chain_id: U256::from(0x66eeeu64),
                    request_id: b256!("0x0100000000000000000000000000000000000000000000000000000000000000"),
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
            ),
            Signature::new(U256::ZERO, U256::ZERO, false),
        );

        let resp: WithOtherFields<EthTransaction<ArbTransactionSigned>> =
            arb_tx_with_other_fields(&tx, signer(), dummy_info());

        let other = &resp.other;
        assert_eq!(other.get_deserialized::<B256>("requestId").unwrap().unwrap(), b256!("0x0100000000000000000000000000000000000000000000000000000000000000"));
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
        use alloy_primitives::Signature;
        let ticket = b256!("0x13cb79b086a427f3db7ebe6ec2bb90a806a3b0368ecee6020144f352e37dbdf6");
        let tx = ArbTransactionSigned::new_unhashed(
            ArbTypedTransaction::Retry(
                arb_alloy_consensus::tx::ArbRetryTx {
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
            ),
            Signature::new(U256::ZERO, U256::ZERO, false),
        );

        let resp: WithOtherFields<EthTransaction<ArbTransactionSigned>> =
            arb_tx_with_other_fields(&tx, signer(), dummy_info());

        let other = &resp.other;
        assert_eq!(other.get_deserialized::<B256>("ticketId").unwrap().unwrap(), ticket);
        assert_eq!(other.get_deserialized::<U256>("maxRefund").unwrap().unwrap(), U256::from_str_radix("b0e85efeab8", 16).unwrap());
        assert_eq!(other.get_deserialized::<U256>("submissionFeeRefund").unwrap().unwrap(), U256::from_str_radix("1f6377d4ab8", 16).unwrap());
        assert_eq!(other.get_deserialized::<Address>("refundTo").unwrap().unwrap(), address!("0x11155ca9bbf7be58e27f3309e629c847996b43c8"));
    }

}

use reth_rpc_convert::transaction::RpcTxConverter;
use core::convert::Infallible;

#[derive(Debug, Clone)]
pub struct ArbRpcTxConverter;

impl RpcTxConverter<
    ArbTransactionSigned,
    WithOtherFields<EthTransaction<ArbTransactionSigned>>,
    TransactionInfo
> for ArbRpcTxConverter {
    type Err = Infallible;

    fn convert_rpc_tx(
        &self,
        tx: ArbTransactionSigned,
        signer: Address,
        tx_info: TransactionInfo
    ) -> Result<WithOtherFields<EthTransaction<ArbTransactionSigned>>, Self::Err> {
        Ok(arb_tx_with_other_fields(&tx, signer, tx_info))
    }
}

use alloy_primitives::Bytes;
use alloy_rpc_types_eth::{Transaction as EthTransaction, TransactionInfo};
use alloy_serde::{OtherFields, WithOtherFields};
use reth_arbitrum_primitives::{ArbTransactionSigned, ArbTypedTransaction};
use reth_primitives_traits::Recovered;
use reth_primitives_traits::SignedTransaction;
use reth_rpc_convert::transaction::FromConsensusTx;
use alloy_consensus::transaction::TxHashRef;

pub fn arb_tx_with_other_fields(
    tx: &ArbTransactionSigned,
    signer: Address,
    mut tx_info: TransactionInfo,
) -> WithOtherFields<EthTransaction<ArbTransactionSigned>> {
    tx_info.hash = Some(*tx.tx_hash());

    let inner = EthTransaction::from_transaction(Recovered::new_unchecked(tx.clone(), signer), tx_info);

    let mut out = WithOtherFields::new(inner);

    match &**tx {
        ArbTypedTransaction::Internal(_) => {
            let _ = out.other.insert_value("type".to_string(), alloy_primitives::hex::encode_prefixed([0x6a]));
            let _ = out.other.insert_value("gas".to_string(), U256::ZERO);
            let _ = out.other.insert_value("gasPrice".to_string(), U256::ZERO);
            let _ = out.other.insert_value("input".to_string(), tx.input().clone());
            let sys_addr = address!("0x00000000000000000000000000000000000a4b05");
            let _ = out.other.insert_value("from".to_string(), sys_addr);
            let _ = out.other.insert_value("to".to_string(), sys_addr);
        }
        ArbTypedTransaction::SubmitRetryable(s) => {
            let _ = out.other.insert_value("type".to_string(), alloy_primitives::hex::encode_prefixed([0x69]));
            let _ = out.other.insert_value("gas".to_string(), U256::from(s.gas));
            let _ = out.other.insert_value("gasPrice".to_string(), s.gas_fee_cap);
            let _ = out.other.insert_value("maxFeePerGas".to_string(), s.gas_fee_cap);
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
            let _ = out.other.insert_value("type".to_string(), alloy_primitives::hex::encode_prefixed([0x68]));
            let _ = out.other.insert_value("gas".to_string(), U256::from(r.gas));
            let _ = out.other.insert_value("gasPrice".to_string(), r.gas_fee_cap);
            let _ = out.other.insert_value("maxFeePerGas".to_string(), r.gas_fee_cap);
            let _ = out.other.insert_value("ticketId".to_string(), r.ticket_id);
            let _ = out.other.insert_value("maxRefund".to_string(), r.max_refund);
            let _ = out.other.insert_value("submissionFeeRefund".to_string(), r.submission_fee_refund);
            let _ = out.other.insert_value("refundTo".to_string(), r.refund_to);
        }
        ArbTypedTransaction::Eip2930(tx2930) => {
            let _ = out.other.insert_value("type".to_string(), alloy_primitives::hex::encode_prefixed([0x01]));
            let _ = out.other.insert_value("gas".to_string(), U256::from(tx2930.gas_limit));
            let _ = out.other.insert_value("gasPrice".to_string(), U256::from(tx2930.gas_price));
            if !tx2930.access_list.0.is_empty() {
                let _ = out.other.insert_value("accessList".to_string(), tx2930.access_list.clone());
            }
            let sig = tx.signature();
            let _ = out.other.insert_value("v".to_string(), U256::from(sig.v() as u64));
            let _ = out.other.insert_value("r".to_string(), sig.r());
            let _ = out.other.insert_value("s".to_string(), sig.s());

        }
        ArbTypedTransaction::Eip1559(tx1559) => {
            let _ = out.other.insert_value("type".to_string(), alloy_primitives::hex::encode_prefixed([0x02]));
            let _ = out.other.insert_value("gas".to_string(), U256::from(tx1559.gas_limit));
            let _ = out.other.insert_value("maxFeePerGas".to_string(), U256::from(tx1559.max_fee_per_gas));
            let _ = out.other.insert_value("maxPriorityFeePerGas".to_string(), U256::from(tx1559.max_priority_fee_per_gas));
            if !tx1559.access_list.0.is_empty() {
                let _ = out.other.insert_value("accessList".to_string(), tx1559.access_list.clone());
            let sig = tx.signature();
            let _ = out.other.insert_value("v".to_string(), U256::from(sig.v() as u64));
            let _ = out.other.insert_value("r".to_string(), sig.r());
            let _ = out.other.insert_value("s".to_string(), sig.s());

            }
        }
        ArbTypedTransaction::Eip4844(tx4844) => {
            let _ = out.other.insert_value("type".to_string(), alloy_primitives::hex::encode_prefixed([0x03]));
            let _ = out.other.insert_value("gas".to_string(), U256::from(tx4844.gas_limit()));
            let _ = out.other.insert_value("maxFeePerGas".to_string(), U256::from(tx4844.max_fee_per_gas()));
            if let Some(mp) = tx4844.max_priority_fee_per_gas() {
                let _ = out.other.insert_value("maxPriorityFeePerGas".to_string(), U256::from(mp));
            }
            if let Some(b) = tx4844.blob_versioned_hashes() {
                let _ = out.other.insert_value("blobVersionedHashes".to_string(), b.clone());
            }
            if let Some(ac) = tx4844.access_list() {
                if !ac.0.is_empty() {
                    let _ = out.other.insert_value("accessList".to_string(), ac.clone());
                }
            let sig = tx.signature();
            let _ = out.other.insert_value("v".to_string(), U256::from(sig.v() as u64));
            let _ = out.other.insert_value("r".to_string(), sig.r());
            let _ = out.other.insert_value("s".to_string(), sig.s());

            }
        }
        ArbTypedTransaction::Eip7702(tx7702) => {
            let _ = out.other.insert_value("type".to_string(), alloy_primitives::hex::encode_prefixed([0x04]));
            let _ = out.other.insert_value("gas".to_string(), U256::from(tx7702.gas_limit()));
            if let Some(gp) = tx7702.gas_price() {
                let _ = out.other.insert_value("gasPrice".to_string(), U256::from(gp));
            }
            {
                let mf = tx7702.max_fee_per_gas();
                let _ = out.other.insert_value("maxFeePerGas".to_string(), U256::from(mf));
            }
            if let Some(mp) = tx7702.max_priority_fee_per_gas() {
                let _ = out.other.insert_value("maxPriorityFeePerGas".to_string(), U256::from(mp));
            }
            if let Some(ac) = tx7702.access_list() {
                if !ac.0.is_empty() {
                    let _ = out.other.insert_value("accessList".to_string(), ac.clone());
                }
            }
            if let Some(auth) = tx7702.authorization_list() {
                if !auth.is_empty() {
                    let _ = out.other.insert_value("authorizationList".to_string(), auth.to_vec());
                }
            let sig = tx.signature();
            let _ = out.other.insert_value("v".to_string(), U256::from(sig.v() as u64));
            let _ = out.other.insert_value("r".to_string(), sig.r());
            let _ = out.other.insert_value("s".to_string(), sig.s());

            }
        }
        _ => {}
    }

    out
}
