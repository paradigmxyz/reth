use alloy_consensus::crypto::secp256k1::recover_signer;
use alloy_eips::{Encodable2718, Typed2718};
use alloy_evm::{FromRecoveredTx, FromTxWithEncoded, IntoTxEnv};
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use core::ops::{Deref, DerefMut};
use revm::context::{
    either::Either,
    transaction::{RecoveredAuthority, RecoveredAuthorization},
    Transaction, TxEnv,
};
use revm_scroll::ScrollTransaction;
use scroll_alloy_consensus::{ScrollTxEnvelope, TxL1Message, L1_MESSAGE_TRANSACTION_TYPE};

mod compression;
pub use compression::{
    compute_compression_ratio, FromTxWithCompressionRatio, ToTxWithCompressionRatio,
    WithCompressionRatio,
};

/// This structure wraps around a [`ScrollTransaction`] and allows us to implement the [`IntoTxEnv`]
/// trait. This can be removed when the interface is improved. Without this wrapper, we would need
/// to implement the trait in `revm-scroll`, which adds a dependency on `alloy-evm` in the crate.
/// Any changes to `alloy-evm` would require changes to `revm-scroll` which isn't desired.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ScrollTransactionIntoTxEnv<T: Transaction>(ScrollTransaction<T>);

impl<T: Transaction> ScrollTransactionIntoTxEnv<T> {
    /// Returns a new [`ScrollTransactionIntoTxEnv`].
    pub fn new(base: T, rlp_bytes: Option<Bytes>, compression_ratio: Option<U256>) -> Self {
        Self(ScrollTransaction::new(base, rlp_bytes, compression_ratio))
    }
}

impl<T: Transaction> From<ScrollTransaction<T>> for ScrollTransactionIntoTxEnv<T> {
    fn from(value: ScrollTransaction<T>) -> Self {
        Self(value)
    }
}

impl<T: Transaction> From<ScrollTransactionIntoTxEnv<T>> for ScrollTransaction<T> {
    fn from(value: ScrollTransactionIntoTxEnv<T>) -> Self {
        value.0
    }
}

impl<T: Transaction> Deref for ScrollTransactionIntoTxEnv<T> {
    type Target = ScrollTransaction<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Transaction> DerefMut for ScrollTransactionIntoTxEnv<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T: Transaction> IntoTxEnv<Self> for ScrollTransactionIntoTxEnv<T> {
    fn into_tx_env(self) -> Self {
        self
    }
}

impl<T: Transaction> Transaction for ScrollTransactionIntoTxEnv<T> {
    type AccessListItem<'a>
        = T::AccessListItem<'a>
    where
        T: 'a;
    type Authorization<'a>
        = T::Authorization<'a>
    where
        T: 'a;

    fn tx_type(&self) -> u8 {
        self.0.tx_type()
    }

    fn caller(&self) -> Address {
        self.0.caller()
    }

    fn gas_limit(&self) -> u64 {
        self.0.gas_limit()
    }

    fn value(&self) -> U256 {
        self.0.value()
    }

    fn input(&self) -> &Bytes {
        self.0.input()
    }

    fn nonce(&self) -> u64 {
        self.0.nonce()
    }

    fn kind(&self) -> TxKind {
        self.0.kind()
    }

    fn chain_id(&self) -> Option<u64> {
        self.0.chain_id()
    }

    fn gas_price(&self) -> u128 {
        self.0.gas_price()
    }

    fn access_list(
        &self,
    ) -> Option<impl Iterator<Item = <ScrollTransaction<T> as Transaction>::AccessListItem<'_>>>
    {
        self.0.access_list()
    }

    fn blob_versioned_hashes(&self) -> &[B256] {
        self.0.blob_versioned_hashes()
    }

    fn max_fee_per_blob_gas(&self) -> u128 {
        self.0.max_fee_per_blob_gas()
    }

    fn authorization_list_len(&self) -> usize {
        self.0.authorization_list_len()
    }

    fn authorization_list(&self) -> impl Iterator<Item = Self::Authorization<'_>> {
        self.0.authorization_list()
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.0.max_priority_fee_per_gas()
    }
}

impl FromTxWithEncoded<ScrollTxEnvelope> for ScrollTransactionIntoTxEnv<TxEnv> {
    fn from_encoded_tx(tx: &ScrollTxEnvelope, caller: Address, encoded: Bytes) -> Self {
        let base = match &tx {
            ScrollTxEnvelope::Legacy(tx) => TxEnv::from_recovered_tx(tx.tx(), caller),
            ScrollTxEnvelope::Eip2930(tx) => TxEnv::from_recovered_tx(tx.tx(), caller),
            ScrollTxEnvelope::Eip1559(tx) => TxEnv::from_recovered_tx(tx.tx(), caller),
            ScrollTxEnvelope::Eip7702(tx) => TxEnv::from_recovered_tx(tx.tx(), caller),
            ScrollTxEnvelope::L1Message(tx) => {
                let TxL1Message { to, value, gas_limit, input, queue_index: _, sender: _ } = &**tx;
                TxEnv {
                    tx_type: tx.ty(),
                    caller,
                    gas_limit: *gas_limit,
                    kind: TxKind::Call(*to),
                    value: *value,
                    data: input.clone(),
                    ..Default::default()
                }
            }
        };

        let encoded = (!tx.is_l1_message()).then_some(encoded);
        // Note: We compute the transaction ratio on tx.data, not on the full encoded transaction.
        let compression_ratio = compute_compression_ratio(base.input());
        Self::new(base, encoded, Some(compression_ratio))
    }
}

impl FromRecoveredTx<ScrollTxEnvelope> for ScrollTransactionIntoTxEnv<TxEnv> {
    fn from_recovered_tx(tx: &ScrollTxEnvelope, sender: Address) -> Self {
        let envelope = tx.encoded_2718();

        let base = match &tx {
            ScrollTxEnvelope::Legacy(tx) => TxEnv {
                gas_limit: tx.tx().gas_limit,
                gas_price: tx.tx().gas_price,
                gas_priority_fee: None,
                kind: tx.tx().to,
                value: tx.tx().value,
                data: tx.tx().input.clone(),
                chain_id: tx.tx().chain_id,
                nonce: tx.tx().nonce,
                access_list: Default::default(),
                blob_hashes: Default::default(),
                max_fee_per_blob_gas: Default::default(),
                authorization_list: Default::default(),
                tx_type: 0,
                caller: sender,
            },
            ScrollTxEnvelope::Eip2930(tx) => TxEnv {
                gas_limit: tx.tx().gas_limit,
                gas_price: tx.tx().gas_price,
                gas_priority_fee: None,
                kind: tx.tx().to,
                value: tx.tx().value,
                data: tx.tx().input.clone(),
                chain_id: Some(tx.tx().chain_id),
                nonce: tx.tx().nonce,
                access_list: tx.tx().access_list.clone(),
                blob_hashes: Default::default(),
                max_fee_per_blob_gas: Default::default(),
                authorization_list: Default::default(),
                tx_type: 1,
                caller: sender,
            },
            ScrollTxEnvelope::Eip1559(tx) => TxEnv {
                gas_limit: tx.tx().gas_limit,
                gas_price: tx.tx().max_fee_per_gas,
                gas_priority_fee: Some(tx.tx().max_priority_fee_per_gas),
                kind: tx.tx().to,
                value: tx.tx().value,
                data: tx.tx().input.clone(),
                chain_id: Some(tx.tx().chain_id),
                nonce: tx.tx().nonce,
                access_list: tx.tx().access_list.clone(),
                blob_hashes: Default::default(),
                max_fee_per_blob_gas: Default::default(),
                authorization_list: Default::default(),
                tx_type: 2,
                caller: sender,
            },
            ScrollTxEnvelope::Eip7702(tx) => TxEnv {
                gas_limit: tx.tx().gas_limit,
                gas_price: tx.tx().max_fee_per_gas,
                gas_priority_fee: Some(tx.tx().max_priority_fee_per_gas),
                kind: tx.tx().to.into(),
                value: tx.tx().value,
                data: tx.tx().input.clone(),
                chain_id: Some(tx.tx().chain_id),
                nonce: tx.tx().nonce,
                access_list: tx.tx().access_list.clone(),
                blob_hashes: Default::default(),
                max_fee_per_blob_gas: Default::default(),
                authorization_list: tx
                    .tx()
                    .authorization_list
                    .iter()
                    .map(|auth| {
                        Either::Right(RecoveredAuthorization::new_unchecked(
                            auth.inner().clone(),
                            auth.signature()
                                .ok()
                                .and_then(|signature| {
                                    recover_signer(&signature, auth.signature_hash()).ok()
                                })
                                .map_or(RecoveredAuthority::Invalid, RecoveredAuthority::Valid),
                        ))
                    })
                    .collect(),
                tx_type: 4,
                caller: sender,
            },
            ScrollTxEnvelope::L1Message(tx) => TxEnv {
                gas_limit: tx.gas_limit,
                gas_price: 0,
                gas_priority_fee: None,
                kind: TxKind::Call(tx.to),
                value: tx.value,
                data: tx.input.clone(),
                chain_id: None,
                nonce: 0,
                access_list: Default::default(),
                blob_hashes: Default::default(),
                max_fee_per_blob_gas: Default::default(),
                authorization_list: Default::default(),
                tx_type: L1_MESSAGE_TRANSACTION_TYPE,
                caller: sender,
            },
        };

        let encoded = (!tx.is_l1_message()).then_some(envelope.into());
        // Note: We compute the transaction ratio on tx.data, not on the full encoded transaction.
        let compression_ratio = compute_compression_ratio(base.input());
        Self::new(base, encoded, Some(compression_ratio))
    }
}
