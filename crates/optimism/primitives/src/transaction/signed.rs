//! A signed Optimism transaction.

use crate::{OpTransaction, OpTxType};
use alloc::vec::Vec;
use alloy_consensus::{
    transaction::RlpEcdsaTx, SignableTransaction, Transaction, TxEip1559, TxEip2930, TxEip7702,
    Typed2718,
};
use alloy_eips::{
    eip2718::{Decodable2718, Eip2718Error, Eip2718Result, Encodable2718},
    eip2930::AccessList,
    eip7702::SignedAuthorization,
};
use alloy_primitives::{
    keccak256, Address, Bytes, PrimitiveSignature as Signature, TxHash, TxKind, Uint, B256, U256,
};
use alloy_rlp::Header;
use core::{
    hash::{Hash, Hasher},
    mem,
};
use derive_more::{AsRef, Deref};
#[cfg(not(feature = "std"))]
use once_cell::sync::OnceCell as OnceLock;
use op_alloy_consensus::{OpTypedTransaction, TxDeposit};
#[cfg(any(test, feature = "reth-codec"))]
use proptest as _;
use reth_primitives::{
    transaction::{recover_signer, recover_signer_unchecked},
    TransactionSigned,
};
use reth_primitives_traits::{FillTxEnv, InMemorySize, SignedTransaction};
use revm_primitives::{AuthorizationList, OptimismFields, TxEnv};
#[cfg(feature = "std")]
use std::sync::OnceLock;

/// Signed transaction.
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(rlp))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Eq, AsRef, Deref)]
pub struct OpTransactionSigned {
    /// Transaction hash
    #[cfg_attr(feature = "serde", serde(skip))]
    pub hash: OnceLock<TxHash>,
    /// The transaction signature values
    pub signature: Signature,
    /// Raw transaction info
    #[deref]
    #[as_ref]
    pub transaction: OpTransaction,
}

impl OpTransactionSigned {
    /// Calculates hash of given transaction and signature and returns new instance.
    pub fn new(transaction: OpTypedTransaction, signature: Signature) -> Self {
        let signed_tx = Self::new_unhashed(transaction, signature);
        if signed_tx.ty() != OpTxType::Deposit {
            signed_tx.hash.get_or_init(|| signed_tx.recalculate_hash());
        }

        signed_tx
    }

    /// Creates a new signed transaction from the given transaction and signature without the hash.
    ///
    /// Note: this only calculates the hash on the first [`TransactionSigned::hash`] call.
    pub fn new_unhashed(transaction: OpTypedTransaction, signature: Signature) -> Self {
        Self { hash: Default::default(), signature, transaction: OpTransaction::new(transaction) }
    }
}

impl SignedTransaction for OpTransactionSigned {
    fn tx_hash(&self) -> &TxHash {
        self.hash.get_or_init(|| self.recalculate_hash())
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn recover_signer(&self) -> Option<Address> {
        // Optimism's Deposit transaction does not have a signature. Directly return the
        // `from` address.
        if let OpTypedTransaction::Deposit(TxDeposit { from, .. }) = *self.transaction {
            return Some(from)
        }

        let Self { transaction, signature, .. } = self;
        let signature_hash = signature_hash(transaction);
        recover_signer(signature, signature_hash)
    }

    fn recover_signer_unchecked(&self) -> Option<Address> {
        // Optimism's Deposit transaction does not have a signature. Directly return the
        // `from` address.
        if let OpTypedTransaction::Deposit(TxDeposit { from, .. }) = *self.transaction {
            return Some(from)
        }

        let Self { transaction, signature, .. } = self;
        let signature_hash = signature_hash(transaction);
        recover_signer_unchecked(signature, signature_hash)
    }

    fn recover_signer_unchecked_with_buf(&self, buf: &mut Vec<u8>) -> Option<Address> {
        // Optimism's Deposit transaction does not have a signature. Directly return the
        // `from` address.
        if let OpTypedTransaction::Deposit(TxDeposit { from, .. }) = *self.transaction {
            return Some(from)
        }
        self.encode_for_signing(buf);
        let signature_hash = keccak256(buf);
        recover_signer_unchecked(&self.signature, signature_hash)
    }

    fn recalculate_hash(&self) -> B256 {
        keccak256(self.encoded_2718())
    }
}

impl FillTxEnv for OpTransactionSigned {
    fn fill_tx_env(&self, tx_env: &mut TxEnv, sender: Address) {
        let envelope = self.encoded_2718();

        tx_env.caller = sender;
        match self.transaction.deref() {
            OpTypedTransaction::Legacy(tx) => {
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::from(tx.gas_price);
                tx_env.gas_priority_fee = None;
                tx_env.transact_to = tx.to;
                tx_env.value = tx.value;
                tx_env.data = tx.input.clone();
                tx_env.chain_id = tx.chain_id;
                tx_env.nonce = Some(tx.nonce);
                tx_env.access_list.clear();
                tx_env.blob_hashes.clear();
                tx_env.max_fee_per_blob_gas.take();
                tx_env.authorization_list = None;
            }
            OpTypedTransaction::Eip2930(tx) => {
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::from(tx.gas_price);
                tx_env.gas_priority_fee = None;
                tx_env.transact_to = tx.to;
                tx_env.value = tx.value;
                tx_env.data = tx.input.clone();
                tx_env.chain_id = Some(tx.chain_id);
                tx_env.nonce = Some(tx.nonce);
                tx_env.access_list.clone_from(&tx.access_list.0);
                tx_env.blob_hashes.clear();
                tx_env.max_fee_per_blob_gas.take();
                tx_env.authorization_list = None;
            }
            OpTypedTransaction::Eip1559(tx) => {
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::from(tx.max_fee_per_gas);
                tx_env.gas_priority_fee = Some(U256::from(tx.max_priority_fee_per_gas));
                tx_env.transact_to = tx.to;
                tx_env.value = tx.value;
                tx_env.data = tx.input.clone();
                tx_env.chain_id = Some(tx.chain_id);
                tx_env.nonce = Some(tx.nonce);
                tx_env.access_list.clone_from(&tx.access_list.0);
                tx_env.blob_hashes.clear();
                tx_env.max_fee_per_blob_gas.take();
                tx_env.authorization_list = None;
            }
            OpTypedTransaction::Eip7702(tx) => {
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::from(tx.max_fee_per_gas);
                tx_env.gas_priority_fee = Some(U256::from(tx.max_priority_fee_per_gas));
                tx_env.transact_to = tx.to.into();
                tx_env.value = tx.value;
                tx_env.data = tx.input.clone();
                tx_env.chain_id = Some(tx.chain_id);
                tx_env.nonce = Some(tx.nonce);
                tx_env.access_list.clone_from(&tx.access_list.0);
                tx_env.blob_hashes.clear();
                tx_env.max_fee_per_blob_gas.take();
                tx_env.authorization_list =
                    Some(AuthorizationList::Signed(tx.authorization_list.clone()));
            }
            OpTypedTransaction::Deposit(tx) => {
                tx_env.access_list.clear();
                tx_env.gas_limit = tx.gas_limit;
                tx_env.gas_price = U256::ZERO;
                tx_env.gas_priority_fee = None;
                tx_env.transact_to = tx.to;
                tx_env.value = tx.value;
                tx_env.data = tx.input.clone();
                tx_env.chain_id = None;
                tx_env.nonce = None;
                tx_env.authorization_list = None;

                tx_env.optimism = OptimismFields {
                    source_hash: Some(tx.source_hash),
                    mint: tx.mint,
                    is_system_transaction: Some(tx.is_system_transaction),
                    enveloped_tx: Some(envelope.into()),
                };
                return
            }
        }

        tx_env.optimism = OptimismFields {
            source_hash: None,
            mint: None,
            is_system_transaction: Some(false),
            enveloped_tx: Some(envelope.into()),
        }
    }
}

impl InMemorySize for OpTransactionSigned {
    #[inline]
    fn size(&self) -> usize {
        mem::size_of::<TxHash>() + self.transaction.size() + mem::size_of::<Signature>()
    }
}

impl alloy_rlp::Encodable for OpTransactionSigned {
    /// See [`alloy_rlp::Encodable`] impl for [`TransactionSigned`].
    fn encode(&self, out: &mut dyn alloy_rlp::bytes::BufMut) {
        self.network_encode(out);
    }

    fn length(&self) -> usize {
        let mut payload_length = self.encode_2718_len();
        if !Encodable2718::is_legacy(self) {
            payload_length += Header { list: false, payload_length }.length();
        }

        payload_length
    }
}

impl alloy_rlp::Decodable for OpTransactionSigned {
    /// See [`alloy_rlp::Decodable`] impl for [`TransactionSigned`].
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Self::network_decode(buf).map_err(Into::into)
    }
}

impl Encodable2718 for OpTransactionSigned {
    fn type_flag(&self) -> Option<u8> {
        if Typed2718::is_legacy(self) {
            None
        } else {
            Some(self.ty())
        }
    }

    fn encode_2718_len(&self) -> usize {
        match self.transaction.deref() {
            OpTypedTransaction::Legacy(legacy_tx) => {
                legacy_tx.eip2718_encoded_length(&self.signature)
            }
            OpTypedTransaction::Eip2930(access_list_tx) => {
                access_list_tx.eip2718_encoded_length(&self.signature)
            }
            OpTypedTransaction::Eip1559(dynamic_fee_tx) => {
                dynamic_fee_tx.eip2718_encoded_length(&self.signature)
            }
            OpTypedTransaction::Eip7702(set_code_tx) => {
                set_code_tx.eip2718_encoded_length(&self.signature)
            }
            OpTypedTransaction::Deposit(deposit_tx) => deposit_tx.eip2718_encoded_length(),
        }
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        let Self { transaction, signature, .. } = self;

        match transaction.deref() {
            OpTypedTransaction::Legacy(legacy_tx) => {
                // do nothing w/ with_header
                legacy_tx.eip2718_encode(signature, out)
            }
            OpTypedTransaction::Eip2930(access_list_tx) => {
                access_list_tx.eip2718_encode(signature, out)
            }
            OpTypedTransaction::Eip1559(dynamic_fee_tx) => {
                dynamic_fee_tx.eip2718_encode(signature, out)
            }
            OpTypedTransaction::Eip7702(set_code_tx) => set_code_tx.eip2718_encode(signature, out),
            OpTypedTransaction::Deposit(deposit_tx) => deposit_tx.encode_2718(out),
        }
    }
}

impl Decodable2718 for OpTransactionSigned {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        match ty.try_into().map_err(|_| Eip2718Error::UnexpectedType(ty))? {
            op_alloy_consensus::OpTxType::Legacy => Err(Eip2718Error::UnexpectedType(0)),
            op_alloy_consensus::OpTxType::Eip2930 => {
                let (tx, signature, hash) = TxEip2930::rlp_decode_signed(buf)?.into_parts();
                let signed_tx = Self::new_unhashed(OpTypedTransaction::Eip2930(tx), signature);
                signed_tx.hash.get_or_init(|| hash);
                Ok(signed_tx)
            }
            op_alloy_consensus::OpTxType::Eip1559 => {
                let (tx, signature, hash) = TxEip1559::rlp_decode_signed(buf)?.into_parts();
                let signed_tx = Self::new_unhashed(OpTypedTransaction::Eip1559(tx), signature);
                signed_tx.hash.get_or_init(|| hash);
                Ok(signed_tx)
            }
            op_alloy_consensus::OpTxType::Eip7702 => {
                let (tx, signature, hash) = TxEip7702::rlp_decode_signed(buf)?.into_parts();
                let signed_tx = Self::new_unhashed(OpTypedTransaction::Eip7702(tx), signature);
                signed_tx.hash.get_or_init(|| hash);
                Ok(signed_tx)
            }
            op_alloy_consensus::OpTxType::Deposit => Ok(Self::new_unhashed(
                OpTypedTransaction::Deposit(TxDeposit::rlp_decode(buf)?),
                TxDeposit::signature(),
            )),
        }
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        let (transaction, hash, signature) =
            TransactionSigned::decode_rlp_legacy_transaction_tuple(buf)?;
        let signed_tx = Self::new_unhashed(OpTypedTransaction::Legacy(transaction), signature);
        signed_tx.hash.get_or_init(|| hash);

        Ok(signed_tx)
    }
}

impl Transaction for OpTransactionSigned {
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

    fn is_dynamic_fee(&self) -> bool {
        self.deref().is_dynamic_fee()
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        self.deref().effective_gas_price(base_fee)
    }

    fn effective_tip_per_gas(&self, base_fee: u64) -> Option<u128> {
        self.deref().effective_tip_per_gas(base_fee)
    }
}

impl Typed2718 for OpTransactionSigned {
    fn ty(&self) -> u8 {
        self.deref().ty()
    }
}

impl Default for OpTransactionSigned {
    fn default() -> Self {
        Self {
            hash: Default::default(),
            signature: Signature::test_signature(),
            transaction: OpTransaction::new(OpTypedTransaction::Legacy(Default::default())),
        }
    }
}

impl PartialEq for OpTransactionSigned {
    fn eq(&self, other: &Self) -> bool {
        self.signature == other.signature &&
            self.transaction == other.transaction &&
            self.tx_hash() == other.tx_hash()
    }
}

impl Hash for OpTransactionSigned {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.signature.hash(state);
        self.transaction.hash(state);
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for OpTransactionSigned {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        #[allow(unused_mut)]
        let mut transaction = OpTypedTransaction::arbitrary(u)?;

        let secp = secp256k1::Secp256k1::new();
        let key_pair = secp256k1::Keypair::new(&secp, &mut rand::thread_rng());
        let signature = reth_primitives::transaction::util::secp256k1::sign_message(
            B256::from_slice(&key_pair.secret_bytes()[..]),
            signature_hash(&transaction),
        )
        .unwrap();

        // Both `Some(0)` and `None` values are encoded as empty string byte. This introduces
        // ambiguity in roundtrip tests. Patch the mint value of deposit transaction here, so that
        // it's `None` if zero.
        if let OpTypedTransaction::Deposit(ref mut tx_deposit) = transaction {
            if tx_deposit.mint == Some(0) {
                tx_deposit.mint = None;
            }
        }

        let signature = if is_deposit(&transaction) { TxDeposit::signature() } else { signature };

        Ok(Self::new(transaction, signature))
    }
}

/// Calculates the signing hash for the transaction.
pub fn signature_hash(tx: &OpTypedTransaction) -> B256 {
    match tx {
        OpTypedTransaction::Legacy(tx) => tx.signature_hash(),
        OpTypedTransaction::Eip2930(tx) => tx.signature_hash(),
        OpTypedTransaction::Eip1559(tx) => tx.signature_hash(),
        OpTypedTransaction::Eip7702(tx) => tx.signature_hash(),
        OpTypedTransaction::Deposit(_) => B256::ZERO,
    }
}

/// Returns `true` if transaction is deposit transaction.
pub const fn is_deposit(tx: &OpTypedTransaction) -> bool {
    matches!(tx, OpTypedTransaction::Deposit(_))
}
