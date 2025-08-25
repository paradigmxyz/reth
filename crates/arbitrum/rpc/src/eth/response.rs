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
