//! A signed Scroll transaction.

use crate::ScrollTxType;
use alloc::{vec, vec::Vec};
use core::{
    hash::{Hash, Hasher},
    mem,
    ops::Deref,
};
#[cfg(feature = "std")]
use std::sync::OnceLock;

use alloy_consensus::{
    transaction::{Either, RlpEcdsaDecodableTx, RlpEcdsaEncodableTx, SignerRecoverable},
    SignableTransaction, Signed, Transaction, TxEip1559, TxEip2930, TxEip7702, TxLegacy, Typed2718,
};
use alloy_eips::{
    eip2718::{Decodable2718, Eip2718Error, Eip2718Result, Encodable2718},
    eip2930::AccessList,
    eip7702::{RecoveredAuthority, RecoveredAuthorization, SignedAuthorization},
};
use alloy_evm::{FromRecoveredTx, FromTxWithEncoded};
use alloy_primitives::{keccak256, Address, Bytes, Signature, TxHash, TxKind, Uint, B256};
use alloy_rlp::Header;
#[cfg(feature = "reth-codec")]
use arbitrary as _;
use derive_more::{AsRef, Deref};
#[cfg(not(feature = "std"))]
use once_cell::sync::OnceCell as OnceLock;
#[cfg(any(test, feature = "reth-codec"))]
use proptest as _;
use reth_primitives_traits::{
    crypto::secp256k1::{recover_signer, recover_signer_unchecked},
    transaction::{error::TryFromRecoveredTransactionError, signed::RecoveryError},
    InMemorySize, SignedTransaction,
};
use revm_context::TxEnv;
use scroll_alloy_consensus::{
    ScrollPooledTransaction, ScrollTypedTransaction, TxL1Message, L1_MESSAGE_TRANSACTION_TYPE,
};
use scroll_alloy_evm::ScrollTransactionIntoTxEnv;

/// Signed transaction.
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(rlp))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Eq, AsRef, Deref)]
pub struct ScrollTransactionSigned {
    /// Transaction hash
    #[cfg_attr(feature = "serde", serde(skip))]
    pub hash: OnceLock<TxHash>,
    /// The transaction signature values
    pub signature: Signature,
    /// Raw transaction info
    #[deref]
    #[as_ref]
    pub transaction: ScrollTypedTransaction,
}

impl ScrollTransactionSigned {
    /// Calculates hash of given transaction and signature and returns new instance.
    pub fn new(transaction: ScrollTypedTransaction, signature: Signature) -> Self {
        let signed_tx = Self::new_unhashed(transaction, signature);
        if signed_tx.ty() != ScrollTxType::L1Message {
            signed_tx.hash.get_or_init(|| signed_tx.recalculate_hash());
        }

        signed_tx
    }

    /// Creates a new signed transaction from the given transaction and signature without the hash.
    ///
    /// Note: this only calculates the hash on the first [`ScrollTransactionSigned::hash`] call.
    pub fn new_unhashed(transaction: ScrollTypedTransaction, signature: Signature) -> Self {
        Self { hash: Default::default(), signature, transaction }
    }

    /// Returns whether this transaction is a l1 message.
    pub const fn is_l1_message(&self) -> bool {
        matches!(self.transaction, ScrollTypedTransaction::L1Message(_))
    }
}

impl SignedTransaction for ScrollTransactionSigned {
    fn tx_hash(&self) -> &TxHash {
        self.hash.get_or_init(|| self.recalculate_hash())
    }

    fn recover_signer_unchecked_with_buf(
        &self,
        buf: &mut Vec<u8>,
    ) -> Result<Address, RecoveryError> {
        match &self.transaction {
            // Scroll's L1 message does not have a signature. Directly return the `sender` address.
            ScrollTypedTransaction::Legacy(tx) => tx.encode_for_signing(buf),
            ScrollTypedTransaction::Eip2930(tx) => tx.encode_for_signing(buf),
            ScrollTypedTransaction::Eip1559(tx) => tx.encode_for_signing(buf),
            ScrollTypedTransaction::Eip7702(tx) => tx.encode_for_signing(buf),
            ScrollTypedTransaction::L1Message(tx) => return Ok(tx.sender),
        };
        recover_signer_unchecked(&self.signature, keccak256(buf))
    }

    fn recalculate_hash(&self) -> B256 {
        keccak256(self.encoded_2718())
    }
}

impl SignerRecoverable for ScrollTransactionSigned {
    fn recover_signer(&self) -> Result<Address, RecoveryError> {
        // Scroll's L1 message does not have a signature. Directly return the `sender` address.
        if let ScrollTypedTransaction::L1Message(TxL1Message { sender, .. }) = self.transaction {
            return Ok(sender);
        }

        let Self { transaction, signature, .. } = self;
        let signature_hash = transaction.signature_hash();
        recover_signer(signature, signature_hash)
    }

    fn recover_signer_unchecked(&self) -> Result<Address, RecoveryError> {
        // Scroll's L1 message does not have a signature. Directly return the `sender` address.
        if let ScrollTypedTransaction::L1Message(TxL1Message { sender, .. }) = &self.transaction {
            return Ok(*sender);
        }

        let Self { transaction, signature, .. } = self;
        let signature_hash = transaction.signature_hash();
        recover_signer_unchecked(signature, signature_hash)
    }
}

impl InMemorySize for ScrollTransactionSigned {
    #[inline]
    fn size(&self) -> usize {
        mem::size_of::<TxHash>() + self.transaction.size() + mem::size_of::<Signature>()
    }
}

impl alloy_rlp::Encodable for ScrollTransactionSigned {
    fn encode(&self, out: &mut dyn alloy_rlp::bytes::BufMut) {
        self.network_encode(out);
    }

    fn length(&self) -> usize {
        let mut payload_length = self.encode_2718_len();
        if !self.is_legacy() {
            payload_length += Header { list: false, payload_length }.length();
        }

        payload_length
    }
}

impl alloy_rlp::Decodable for ScrollTransactionSigned {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Self::network_decode(buf).map_err(Into::into)
    }
}

impl Encodable2718 for ScrollTransactionSigned {
    fn type_flag(&self) -> Option<u8> {
        if Typed2718::is_legacy(self) {
            None
        } else {
            Some(self.ty())
        }
    }

    fn encode_2718_len(&self) -> usize {
        match &self.transaction {
            ScrollTypedTransaction::Legacy(legacy_tx) => {
                legacy_tx.eip2718_encoded_length(&self.signature)
            }
            ScrollTypedTransaction::Eip2930(access_list_tx) => {
                access_list_tx.eip2718_encoded_length(&self.signature)
            }
            ScrollTypedTransaction::Eip1559(dynamic_fee_tx) => {
                dynamic_fee_tx.eip2718_encoded_length(&self.signature)
            }
            ScrollTypedTransaction::Eip7702(authorization_list_tx) => {
                authorization_list_tx.eip2718_encoded_length(&self.signature)
            }
            ScrollTypedTransaction::L1Message(l1_message) => l1_message.eip2718_encoded_length(),
        }
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        let Self { transaction, signature, .. } = self;

        match &transaction {
            ScrollTypedTransaction::Legacy(legacy_tx) => {
                // do nothing w/ with_header
                legacy_tx.eip2718_encode(signature, out)
            }
            ScrollTypedTransaction::Eip2930(access_list_tx) => {
                access_list_tx.eip2718_encode(signature, out)
            }
            ScrollTypedTransaction::Eip1559(dynamic_fee_tx) => {
                dynamic_fee_tx.eip2718_encode(signature, out)
            }
            ScrollTypedTransaction::Eip7702(authorization_list_tx) => {
                authorization_list_tx.eip2718_encode(signature, out)
            }
            ScrollTypedTransaction::L1Message(l1_message) => l1_message.encode_2718(out),
        }
    }
}

impl Decodable2718 for ScrollTransactionSigned {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        match ty.try_into().map_err(|_| Eip2718Error::UnexpectedType(ty))? {
            ScrollTxType::Legacy => Err(Eip2718Error::UnexpectedType(0)),
            ScrollTxType::Eip2930 => {
                let (tx, signature, hash) = TxEip2930::rlp_decode_signed(buf)?.into_parts();
                let signed_tx = Self::new_unhashed(ScrollTypedTransaction::Eip2930(tx), signature);
                signed_tx.hash.get_or_init(|| hash);
                Ok(signed_tx)
            }
            ScrollTxType::Eip1559 => {
                let (tx, signature, hash) = TxEip1559::rlp_decode_signed(buf)?.into_parts();
                let signed_tx = Self::new_unhashed(ScrollTypedTransaction::Eip1559(tx), signature);
                signed_tx.hash.get_or_init(|| hash);
                Ok(signed_tx)
            }
            ScrollTxType::Eip7702 => {
                let (tx, signature, hash) = TxEip7702::rlp_decode_signed(buf)?.into_parts();
                let signed_tx = Self::new_unhashed(ScrollTypedTransaction::Eip7702(tx), signature);
                signed_tx.hash.get_or_init(|| hash);
                Ok(signed_tx)
            }
            ScrollTxType::L1Message => Ok(Self::new_unhashed(
                ScrollTypedTransaction::L1Message(TxL1Message::rlp_decode(buf)?),
                TxL1Message::signature(),
            )),
        }
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        let (transaction, signature) = TxLegacy::rlp_decode_with_signature(buf)?;
        let signed_tx = Self::new_unhashed(ScrollTypedTransaction::Legacy(transaction), signature);

        Ok(signed_tx)
    }
}

impl Transaction for ScrollTransactionSigned {
    fn chain_id(&self) -> Option<u64> {
        self.deref().chain_id()
    }

    fn nonce(&self) -> u64 {
        self.deref().nonce()
    }

    fn gas_limit(&self) -> u64 {
        self.deref().gas_limit()
    }

    fn gas_price(&self) -> Option<u128> {
        self.deref().gas_price()
    }

    fn max_fee_per_gas(&self) -> u128 {
        self.deref().max_fee_per_gas()
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.deref().max_priority_fee_per_gas()
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        self.deref().max_fee_per_blob_gas()
    }

    fn priority_fee_or_price(&self) -> u128 {
        self.deref().priority_fee_or_price()
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        self.deref().effective_gas_price(base_fee)
    }

    fn effective_tip_per_gas(&self, base_fee: u64) -> Option<u128> {
        self.deref().effective_tip_per_gas(base_fee)
    }

    fn is_dynamic_fee(&self) -> bool {
        self.deref().is_dynamic_fee()
    }

    fn kind(&self) -> TxKind {
        self.deref().kind()
    }

    fn is_create(&self) -> bool {
        self.deref().is_create()
    }

    fn value(&self) -> Uint<256, 4> {
        self.deref().value()
    }

    fn input(&self) -> &Bytes {
        self.deref().input()
    }

    fn access_list(&self) -> Option<&AccessList> {
        self.deref().access_list()
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        self.deref().blob_versioned_hashes()
    }

    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        self.deref().authorization_list()
    }
}

/// A trait that allows to verify if a transaction is a l1 message.
pub trait IsL1Message {
    /// Whether the transaction is a l1 transaction.
    fn is_l1_message(&self) -> bool;
}

impl IsL1Message for ScrollTransactionSigned {
    fn is_l1_message(&self) -> bool {
        matches!(self.transaction, ScrollTypedTransaction::L1Message(_))
    }
}

impl Typed2718 for ScrollTransactionSigned {
    fn ty(&self) -> u8 {
        self.deref().ty()
    }
}

impl PartialEq for ScrollTransactionSigned {
    fn eq(&self, other: &Self) -> bool {
        self.signature == other.signature &&
            self.transaction == other.transaction &&
            self.tx_hash() == other.tx_hash()
    }
}

impl Hash for ScrollTransactionSigned {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.signature.hash(state);
        self.transaction.hash(state);
    }
}

impl FromTxWithEncoded<ScrollTransactionSigned> for ScrollTransactionIntoTxEnv<TxEnv> {
    fn from_encoded_tx(tx: &ScrollTransactionSigned, caller: Address, encoded: Bytes) -> Self {
        let base = match &tx.transaction {
            ScrollTypedTransaction::Legacy(tx) => TxEnv::from_recovered_tx(tx, caller),
            ScrollTypedTransaction::Eip2930(tx) => TxEnv::from_recovered_tx(tx, caller),
            ScrollTypedTransaction::Eip1559(tx) => TxEnv::from_recovered_tx(tx, caller),
            ScrollTypedTransaction::Eip7702(tx) => TxEnv::from_recovered_tx(tx, caller),
            ScrollTypedTransaction::L1Message(tx) => {
                let TxL1Message { to, value, gas_limit, input, queue_index: _, sender: _ } = tx;
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
        Self::new(base, encoded)
    }
}

#[cfg(feature = "reth-codec")]
impl reth_codecs::Compact for ScrollTransactionSigned {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let start = buf.as_mut().len();

        // Placeholder for bitflags.
        // The first byte uses 4 bits as flags: IsCompressed[1bit], TxType[2bits], Signature[1bit]
        buf.put_u8(0);

        let sig_bit = self.signature.to_compact(buf) as u8;
        let zstd_bit = self.transaction.input().len() >= 32;

        let tx_bits = if zstd_bit {
            let mut tmp = Vec::with_capacity(256);
            if cfg!(feature = "std") {
                reth_zstd_compressors::TRANSACTION_COMPRESSOR.with(|compressor| {
                    let mut compressor = compressor.borrow_mut();
                    let tx_bits = self.transaction.to_compact(&mut tmp);
                    buf.put_slice(&compressor.compress(&tmp).expect("Failed to compress"));
                    tx_bits as u8
                })
            } else {
                let mut compressor = reth_zstd_compressors::create_tx_compressor();
                let tx_bits = self.transaction.to_compact(&mut tmp);
                buf.put_slice(&compressor.compress(&tmp).expect("Failed to compress"));
                tx_bits as u8
            }
        } else {
            self.transaction.to_compact(buf) as u8
        };

        // Replace bitflags with the actual values
        buf.as_mut()[start] = sig_bit | (tx_bits << 1) | ((zstd_bit as u8) << 3);

        buf.as_mut().len() - start
    }

    fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8]) {
        use bytes::Buf;

        // The first byte uses 4 bits as flags: IsCompressed[1], TxType[2], Signature[1]
        let bitflags = buf.get_u8() as usize;

        let sig_bit = bitflags & 1;
        let (signature, buf) = Signature::from_compact(buf, sig_bit);

        let zstd_bit = bitflags >> 3;
        let (transaction, buf) = if zstd_bit != 0 {
            if cfg!(feature = "std") {
                reth_zstd_compressors::TRANSACTION_DECOMPRESSOR.with(|decompressor| {
                    let mut decompressor = decompressor.borrow_mut();

                    // TODO: enforce that zstd is only present at a "top" level type
                    let transaction_type = (bitflags & 0b110) >> 1;
                    let (transaction, _) = ScrollTypedTransaction::from_compact(
                        decompressor.decompress(buf),
                        transaction_type,
                    );

                    (transaction, buf)
                })
            } else {
                let mut decompressor = reth_zstd_compressors::create_tx_decompressor();
                let transaction_type = (bitflags & 0b110) >> 1;
                let (transaction, _) = ScrollTypedTransaction::from_compact(
                    decompressor.decompress(buf),
                    transaction_type,
                );

                (transaction, buf)
            }
        } else {
            let transaction_type = bitflags >> 1;
            ScrollTypedTransaction::from_compact(buf, transaction_type)
        };

        (Self { signature, transaction, hash: Default::default() }, buf)
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for ScrollTransactionSigned {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        #[allow(unused_mut)]
        let mut transaction = ScrollTypedTransaction::arbitrary(u)?;

        let secp = secp256k1::Secp256k1::new();
        let key_pair = secp256k1::Keypair::new(&secp, &mut rand_08::thread_rng());
        let signature = reth_primitives_traits::crypto::secp256k1::sign_message(
            B256::from_slice(&key_pair.secret_bytes()[..]),
            transaction.signature_hash(),
        )
        .unwrap();

        let signature =
            if is_l1_message(&transaction) { TxL1Message::signature() } else { signature };

        Ok(Self::new(transaction, signature))
    }
}

/// Returns `true` if transaction is l1 message.
pub const fn is_l1_message(tx: &ScrollTypedTransaction) -> bool {
    matches!(tx, ScrollTypedTransaction::L1Message(_))
}

impl<T: Into<ScrollTypedTransaction> + RlpEcdsaEncodableTx> From<Signed<T>>
    for ScrollTransactionSigned
{
    fn from(value: Signed<T>) -> Self {
        let (tx, sig, hash) = value.into_parts();
        let this = Self::new(tx.into(), sig);
        this.hash.get_or_init(|| hash);
        this
    }
}

impl TryFrom<ScrollTransactionSigned> for ScrollPooledTransaction {
    type Error = TryFromRecoveredTransactionError;

    fn try_from(value: ScrollTransactionSigned) -> Result<Self, Self::Error> {
        let hash = *value.tx_hash();
        let ScrollTransactionSigned { hash: _, signature, transaction } = value;

        match transaction {
            ScrollTypedTransaction::Legacy(tx) => {
                Ok(Self::Legacy(Signed::new_unchecked(tx, signature, hash)))
            }
            ScrollTypedTransaction::Eip2930(tx) => {
                Ok(Self::Eip2930(Signed::new_unchecked(tx, signature, hash)))
            }
            ScrollTypedTransaction::Eip1559(tx) => {
                Ok(Self::Eip1559(Signed::new_unchecked(tx, signature, hash)))
            }
            ScrollTypedTransaction::Eip7702(tx) => {
                Ok(Self::Eip7702(Signed::new_unchecked(tx, signature, hash)))
            }
            ScrollTypedTransaction::L1Message(_) => {
                Err(TryFromRecoveredTransactionError::UnsupportedTransactionType(0xfe))
            }
        }
    }
}

impl From<ScrollPooledTransaction> for ScrollTransactionSigned {
    fn from(value: ScrollPooledTransaction) -> Self {
        match value {
            ScrollPooledTransaction::Legacy(tx) => tx.into(),
            ScrollPooledTransaction::Eip2930(tx) => tx.into(),
            ScrollPooledTransaction::Eip1559(tx) => tx.into(),
            ScrollPooledTransaction::Eip7702(tx) => tx.into(),
        }
    }
}

impl FromRecoveredTx<ScrollTransactionSigned> for revm_scroll::ScrollTransaction<TxEnv> {
    fn from_recovered_tx(tx: &ScrollTransactionSigned, sender: Address) -> Self {
        let envelope = tx.encoded_2718();

        let base = match &tx.transaction {
            ScrollTypedTransaction::Legacy(tx) => TxEnv {
                gas_limit: tx.gas_limit,
                gas_price: tx.gas_price,
                gas_priority_fee: None,
                kind: tx.to,
                value: tx.value,
                data: tx.input.clone(),
                chain_id: tx.chain_id,
                nonce: tx.nonce,
                access_list: Default::default(),
                blob_hashes: Default::default(),
                max_fee_per_blob_gas: Default::default(),
                authorization_list: Default::default(),
                tx_type: 0,
                caller: sender,
            },
            ScrollTypedTransaction::Eip2930(tx) => TxEnv {
                gas_limit: tx.gas_limit,
                gas_price: tx.gas_price,
                gas_priority_fee: None,
                kind: tx.to,
                value: tx.value,
                data: tx.input.clone(),
                chain_id: Some(tx.chain_id),
                nonce: tx.nonce,
                access_list: tx.access_list.clone(),
                blob_hashes: Default::default(),
                max_fee_per_blob_gas: Default::default(),
                authorization_list: Default::default(),
                tx_type: 1,
                caller: sender,
            },
            ScrollTypedTransaction::Eip1559(tx) => TxEnv {
                gas_limit: tx.gas_limit,
                gas_price: tx.max_fee_per_gas,
                gas_priority_fee: Some(tx.max_priority_fee_per_gas),
                kind: tx.to,
                value: tx.value,
                data: tx.input.clone(),
                chain_id: Some(tx.chain_id),
                nonce: tx.nonce,
                access_list: tx.access_list.clone(),
                blob_hashes: Default::default(),
                max_fee_per_blob_gas: Default::default(),
                authorization_list: Default::default(),
                tx_type: 2,
                caller: sender,
            },
            ScrollTypedTransaction::Eip7702(tx) => TxEnv {
                gas_limit: tx.gas_limit,
                gas_price: tx.max_fee_per_gas,
                gas_priority_fee: Some(tx.max_priority_fee_per_gas),
                kind: tx.to.into(),
                value: tx.value,
                data: tx.input.clone(),
                chain_id: Some(tx.chain_id),
                nonce: tx.nonce,
                access_list: tx.access_list.clone(),
                blob_hashes: Default::default(),
                max_fee_per_blob_gas: Default::default(),
                authorization_list: tx
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
            ScrollTypedTransaction::L1Message(tx) => TxEnv {
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
                max_fee_per_blob_gas: 0,
                authorization_list: vec![],
                tx_type: L1_MESSAGE_TRANSACTION_TYPE,
                caller: sender,
            },
        };

        Self { base, rlp_bytes: (!tx.is_l1_message()).then_some(envelope.into()) }
    }
}

impl FromRecoveredTx<ScrollTransactionSigned> for ScrollTransactionIntoTxEnv<TxEnv> {
    fn from_recovered_tx(tx: &ScrollTransactionSigned, sender: Address) -> Self {
        let envelope = tx.encoded_2718();

        let base = match &tx.transaction {
            ScrollTypedTransaction::Legacy(tx) => TxEnv {
                gas_limit: tx.gas_limit,
                gas_price: tx.gas_price,
                gas_priority_fee: None,
                kind: tx.to,
                value: tx.value,
                data: tx.input.clone(),
                chain_id: tx.chain_id,
                nonce: tx.nonce,
                access_list: Default::default(),
                blob_hashes: Default::default(),
                max_fee_per_blob_gas: Default::default(),
                authorization_list: Default::default(),
                tx_type: 0,
                caller: sender,
            },
            ScrollTypedTransaction::Eip2930(tx) => TxEnv {
                gas_limit: tx.gas_limit,
                gas_price: tx.gas_price,
                gas_priority_fee: None,
                kind: tx.to,
                value: tx.value,
                data: tx.input.clone(),
                chain_id: Some(tx.chain_id),
                nonce: tx.nonce,
                access_list: tx.access_list.clone(),
                blob_hashes: Default::default(),
                max_fee_per_blob_gas: Default::default(),
                authorization_list: Default::default(),
                tx_type: 1,
                caller: sender,
            },
            ScrollTypedTransaction::Eip1559(tx) => TxEnv {
                gas_limit: tx.gas_limit,
                gas_price: tx.max_fee_per_gas,
                gas_priority_fee: Some(tx.max_priority_fee_per_gas),
                kind: tx.to,
                value: tx.value,
                data: tx.input.clone(),
                chain_id: Some(tx.chain_id),
                nonce: tx.nonce,
                access_list: tx.access_list.clone(),
                blob_hashes: Default::default(),
                max_fee_per_blob_gas: Default::default(),
                authorization_list: Default::default(),
                tx_type: 2,
                caller: sender,
            },
            ScrollTypedTransaction::Eip7702(tx) => TxEnv {
                gas_limit: tx.gas_limit,
                gas_price: tx.max_fee_per_gas,
                gas_priority_fee: Some(tx.max_priority_fee_per_gas),
                kind: tx.to.into(),
                value: tx.value,
                data: tx.input.clone(),
                chain_id: Some(tx.chain_id),
                nonce: tx.nonce,
                access_list: tx.access_list.clone(),
                blob_hashes: Default::default(),
                max_fee_per_blob_gas: Default::default(),
                authorization_list: tx
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
            ScrollTypedTransaction::L1Message(tx) => TxEnv {
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

        let rlp_bytes = (!tx.is_l1_message()).then_some(envelope.into());
        Self::new(base, rlp_bytes)
    }
}

/// Bincode-compatible transaction type serde implementations.
#[cfg(feature = "serde-bincode-compat")]
pub mod serde_bincode_compat {
    use alloc::borrow::Cow;
    use alloy_consensus::transaction::serde_bincode_compat::{
        TxEip1559, TxEip2930, TxEip7702, TxLegacy,
    };
    use alloy_primitives::{Signature, TxHash};
    use reth_primitives_traits::{serde_bincode_compat::SerdeBincodeCompat, SignedTransaction};
    use serde::{Deserialize, Serialize};

    /// Bincode-compatible [`super::ScrollTypedTransaction`] serde implementation.
    #[derive(Debug, Serialize, Deserialize)]
    #[allow(missing_docs)]
    enum ScrollTypedTransaction<'a> {
        Legacy(TxLegacy<'a>),
        Eip2930(TxEip2930<'a>),
        Eip1559(TxEip1559<'a>),
        Eip7702(TxEip7702<'a>),
        L1Message(Cow<'a, scroll_alloy_consensus::TxL1Message>),
    }

    impl<'a> From<&'a super::ScrollTypedTransaction> for ScrollTypedTransaction<'a> {
        fn from(value: &'a super::ScrollTypedTransaction) -> Self {
            match value {
                super::ScrollTypedTransaction::Legacy(tx) => Self::Legacy(TxLegacy::from(tx)),
                super::ScrollTypedTransaction::Eip2930(tx) => Self::Eip2930(TxEip2930::from(tx)),
                super::ScrollTypedTransaction::Eip1559(tx) => Self::Eip1559(TxEip1559::from(tx)),
                super::ScrollTypedTransaction::Eip7702(tx) => Self::Eip7702(TxEip7702::from(tx)),
                super::ScrollTypedTransaction::L1Message(tx) => Self::L1Message(Cow::Borrowed(tx)),
            }
        }
    }

    impl<'a> From<ScrollTypedTransaction<'a>> for super::ScrollTypedTransaction {
        fn from(value: ScrollTypedTransaction<'a>) -> Self {
            match value {
                ScrollTypedTransaction::Legacy(tx) => Self::Legacy(tx.into()),
                ScrollTypedTransaction::Eip2930(tx) => Self::Eip2930(tx.into()),
                ScrollTypedTransaction::Eip1559(tx) => Self::Eip1559(tx.into()),
                ScrollTypedTransaction::Eip7702(tx) => Self::Eip7702(tx.into()),
                ScrollTypedTransaction::L1Message(tx) => Self::L1Message(tx.into_owned()),
            }
        }
    }

    /// Bincode-compatible [`super::ScrollTransactionSigned`] serde implementation.
    #[derive(Debug, Serialize, Deserialize)]
    pub struct ScrollTransactionSigned<'a> {
        hash: TxHash,
        signature: Signature,
        transaction: ScrollTypedTransaction<'a>,
    }

    impl<'a> From<&'a super::ScrollTransactionSigned> for ScrollTransactionSigned<'a> {
        fn from(value: &'a super::ScrollTransactionSigned) -> Self {
            Self {
                hash: *value.tx_hash(),
                signature: value.signature,
                transaction: ScrollTypedTransaction::from(&value.transaction),
            }
        }
    }

    impl<'a> From<ScrollTransactionSigned<'a>> for super::ScrollTransactionSigned {
        fn from(value: ScrollTransactionSigned<'a>) -> Self {
            Self {
                hash: value.hash.into(),
                signature: value.signature,
                transaction: value.transaction.into(),
            }
        }
    }

    impl SerdeBincodeCompat for super::ScrollTransactionSigned {
        type BincodeRepr<'a> = ScrollTransactionSigned<'a>;

        fn as_repr(&self) -> Self::BincodeRepr<'_> {
            self.into()
        }

        fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
            repr.into()
        }
    }
}
