#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256, bytes, Address, B256, Bytes, U256};
    use reth_arbitrum_primitives::ArbTypedTransaction;
    use reth_rpc_convert::transaction::FromConsensusTx as _;

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

        let resp = ArbTransactionResponse::from_consensus_tx(tx, signer(), dummy_info());
        assert_eq!(resp.requestId, Some(b256!("0x01")));
        assert_eq!(resp.refundTo, Some(address!("0x11155ca9bbf7be58e27f3309e629c847996b43c8")));
        assert_eq!(resp.l1BaseFee, Some(U256::from(0x5bd57bd9u64)));
        assert_eq!(resp.depositValue, Some(U256::from_str_radix("23e3dbb7b88ab8", 16).unwrap()));
        assert_eq!(resp.retryTo, Some(address!("0x3fab184622dc19b6109349b94811493bf2a45362")));
        assert_eq!(resp.retryValue, Some(U256::from_str_radix("2386f26fc10000", 16).unwrap()));
        assert_eq!(resp.retryData, Some(Bytes::default()));
        assert_eq!(resp.beneficiary, Some(address!("0x11155ca9bbf7be58e27f3309e629c847996b43c8")));
        assert_eq!(resp.maxSubmissionFee, Some(U256::from_str_radix("1f6377d4ab8", 16).unwrap()));
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

        let resp = ArbTransactionResponse::from_consensus_tx(tx, signer(), dummy_info());
        assert_eq!(resp.ticketId, Some(ticket));
        assert_eq!(resp.maxRefund, Some(U256::from_str_radix("b0e85efeab8", 16).unwrap()));
        assert_eq!(resp.submissionFeeRefund, Some(U256::from_str_radix("1f6377d4ab8", 16).unwrap()));
        assert_eq!(resp.refundTo, Some(address!("0x11155ca9bbf7be58e27f3309e629c847996b43c8")));
    }
}

use serde::{Deserialize, Serialize};

use alloy_primitives::{Address, Bytes, B256, U256};
use alloy_rpc_types_eth::{Transaction as EthTransaction, TransactionInfo};
use reth_rpc_convert::transaction::FromConsensusTx;

use reth_arbitrum_primitives::{ArbTransactionSigned, ArbTypedTransaction};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArbTransactionResponse {
    #[serde(flatten)]
    pub inner: EthTransaction<ArbTransactionSigned>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub requestId: Option<B256>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub refundTo: Option<Address>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub l1BaseFee: Option<U256>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub depositValue: Option<U256>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryTo: Option<Address>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryValue: Option<U256>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryData: Option<Bytes>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub beneficiary: Option<Address>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub maxSubmissionFee: Option<U256>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub ticketId: Option<B256>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub maxRefund: Option<U256>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub submissionFeeRefund: Option<U256>,
}

impl FromConsensusTx<ArbTransactionSigned> for ArbTransactionResponse {
    type TxInfo = TransactionInfo;

    fn from_consensus_tx(
        tx: ArbTransactionSigned,
        signer: Address,
        tx_info: Self::TxInfo,
    ) -> Self {
        let inner = EthTransaction::from_transaction(reth_rpc_convert::transaction::Recovered::new_unchecked(tx.clone(), signer), tx_info);

        let mut out = ArbTransactionResponse {
            inner,
            requestId: None,
            refundTo: None,
            l1BaseFee: None,
            depositValue: None,
            retryTo: None,
            retryValue: None,
            retryData: None,
            beneficiary: None,
            maxSubmissionFee: None,
            ticketId: None,
            maxRefund: None,
            submissionFeeRefund: None,
        };

        match &*tx {
            ArbTypedTransaction::SubmitRetryable(s) => {
                out.requestId = Some(s.request_id);
                out.refundTo = Some(s.fee_refund_addr);
                out.l1BaseFee = Some(s.l1_base_fee);
                out.depositValue = Some(s.deposit_value);
                out.retryTo = s.retry_to;
                out.retryValue = Some(s.retry_value);
                out.retryData = Some(s.retry_data.clone());
                out.beneficiary = Some(s.beneficiary);
                out.maxSubmissionFee = Some(s.max_submission_fee);
            }
            ArbTypedTransaction::Retry(r) => {
                out.ticketId = Some(r.ticket_id);
                out.maxRefund = Some(r.max_refund);
                out.submissionFeeRefund = Some(r.submission_fee_refund);
                out.refundTo = Some(r.refund_to);
            }
            _ => {}
        }

        out
    }
}
