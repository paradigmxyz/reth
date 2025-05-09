use alloy_consensus::{
    Sealed, SignableTransaction, Signed, TxEip1559, TxEip4844, TypedTransaction,
};
use alloy_primitives::{Address, Signature, TxKind, U256};
use alloy_rpc_types_eth::{AccessList, TransactionInput, TransactionRequest};
use serde::{Deserialize, Serialize};

use scroll_alloy_consensus::{ScrollTxEnvelope, ScrollTypedTransaction, TxL1Message};

/// `ScrollTransactionRequest` is a wrapper around the `TransactionRequest` struct.
/// This struct derives several traits to facilitate easier use and manipulation
/// in the codebase.
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    derive_more::From,
    derive_more::AsRef,
    derive_more::AsMut,
    Serialize,
    Deserialize,
)]
#[serde(transparent)]
pub struct ScrollTransactionRequest(TransactionRequest);

impl ScrollTransactionRequest {
    /// Sets the from field in the call to the provided address
    #[inline]
    pub const fn from(mut self, from: Address) -> Self {
        self.0.from = Some(from);
        self
    }

    /// Sets the transactions type for the transactions.
    #[doc(alias = "tx_type")]
    pub const fn transaction_type(mut self, transaction_type: u8) -> Self {
        self.0.transaction_type = Some(transaction_type);
        self
    }

    /// Sets the gas limit for the transaction.
    pub const fn gas_limit(mut self, gas_limit: u64) -> Self {
        self.0.gas = Some(gas_limit);
        self
    }

    /// Sets the nonce for the transaction.
    pub const fn nonce(mut self, nonce: u64) -> Self {
        self.0.nonce = Some(nonce);
        self
    }

    /// Sets the maximum fee per gas for the transaction.
    pub const fn max_fee_per_gas(mut self, max_fee_per_gas: u128) -> Self {
        self.0.max_fee_per_gas = Some(max_fee_per_gas);
        self
    }

    /// Sets the maximum priority fee per gas for the transaction.
    pub const fn max_priority_fee_per_gas(mut self, max_priority_fee_per_gas: u128) -> Self {
        self.0.max_priority_fee_per_gas = Some(max_priority_fee_per_gas);
        self
    }

    /// Sets the recipient address for the transaction.
    #[inline]
    pub const fn to(mut self, to: Address) -> Self {
        self.0.to = Some(TxKind::Call(to));
        self
    }

    /// Sets the value (amount) for the transaction.
    pub const fn value(mut self, value: U256) -> Self {
        self.0.value = Some(value);
        self
    }

    /// Sets the access list for the transaction.
    pub fn access_list(mut self, access_list: AccessList) -> Self {
        self.0.access_list = Some(access_list);
        self
    }

    /// Sets the input data for the transaction.
    pub fn input(mut self, input: TransactionInput) -> Self {
        self.0.input = input;
        self
    }

    /// Builds [`ScrollTypedTransaction`] from this builder. See
    /// [`TransactionRequest::build_typed_tx`] for more info.
    ///
    /// Note that EIP-4844 transactions are not supported by Scroll and will be converted into
    /// EIP-1559 transactions.
    pub fn build_typed_tx(self) -> Result<ScrollTypedTransaction, Self> {
        let tx = self.0.build_typed_tx().map_err(Self)?;
        match tx {
            TypedTransaction::Legacy(tx) => Ok(ScrollTypedTransaction::Legacy(tx)),
            TypedTransaction::Eip1559(tx) => Ok(ScrollTypedTransaction::Eip1559(tx)),
            TypedTransaction::Eip2930(tx) => Ok(ScrollTypedTransaction::Eip2930(tx)),
            TypedTransaction::Eip4844(tx) => {
                let tx: TxEip4844 = tx.into();
                Ok(ScrollTypedTransaction::Eip1559(TxEip1559 {
                    chain_id: tx.chain_id,
                    nonce: tx.nonce,
                    gas_limit: tx.gas_limit,
                    max_priority_fee_per_gas: tx.max_priority_fee_per_gas,
                    max_fee_per_gas: tx.max_fee_per_gas,
                    to: TxKind::Call(tx.to),
                    value: tx.value,
                    access_list: tx.access_list,
                    input: tx.input,
                }))
            }
            TypedTransaction::Eip7702(_) => {
                unimplemented!("EIP-7702 support is not implemented yet")
            }
        }
    }
}

impl From<TxL1Message> for ScrollTransactionRequest {
    fn from(tx_l1_message: TxL1Message) -> Self {
        let to = TxKind::from(tx_l1_message.to);
        Self(TransactionRequest {
            from: Some(tx_l1_message.sender),
            to: Some(to),
            value: Some(tx_l1_message.value),
            gas: Some(tx_l1_message.gas_limit),
            input: tx_l1_message.input.into(),
            ..Default::default()
        })
    }
}

impl From<Sealed<TxL1Message>> for ScrollTransactionRequest {
    fn from(value: Sealed<TxL1Message>) -> Self {
        value.into_inner().into()
    }
}

impl<T> From<Signed<T, Signature>> for ScrollTransactionRequest
where
    T: SignableTransaction<Signature> + Into<TransactionRequest>,
{
    fn from(value: Signed<T, Signature>) -> Self {
        #[cfg(feature = "k256")]
        let from = value.recover_signer().ok();
        #[cfg(not(feature = "k256"))]
        let from = None;

        let mut inner: TransactionRequest = value.strip_signature().into();
        inner.from = from;

        Self(inner)
    }
}

impl From<ScrollTypedTransaction> for ScrollTransactionRequest {
    fn from(tx: ScrollTypedTransaction) -> Self {
        match tx {
            ScrollTypedTransaction::Legacy(tx) => Self(tx.into()),
            ScrollTypedTransaction::Eip2930(tx) => Self(tx.into()),
            ScrollTypedTransaction::Eip1559(tx) => Self(tx.into()),
            ScrollTypedTransaction::Eip7702(tx) => Self(tx.into()),
            ScrollTypedTransaction::L1Message(tx) => tx.into(),
        }
    }
}

impl From<ScrollTxEnvelope> for ScrollTransactionRequest {
    fn from(value: ScrollTxEnvelope) -> Self {
        match value {
            ScrollTxEnvelope::Eip2930(tx) => tx.into(),
            ScrollTxEnvelope::Eip1559(tx) => tx.into(),
            ScrollTxEnvelope::Eip7702(tx) => tx.into(),
            ScrollTxEnvelope::L1Message(tx) => tx.into(),
            _ => Default::default(),
        }
    }
}
