//! A signed Optimism transaction.

#[cfg(any(test, feature = "arbitrary"))]
use alloy_consensus::Transaction as _;
use alloy_consensus::{SignableTransaction, TxEip1559, TxEip2930, TxEip7702};
use alloy_eips::eip2718::{Decodable2718, Eip2718Error, Eip2718Result, Encodable2718};
use alloy_primitives::{keccak256, Address, Signature, TxHash, B256, U256};
use alloy_rlp::{Decodable as _, Header};
use derive_more::{AsRef, Constructor, Deref};
use op_alloy_consensus::{OpTxType, OpTypedTransaction, TxDeposit};
use reth_primitives::{
    transaction::{recover_signer, recover_signer_unchecked, with_eip155_parity},
    TransactionSigned,
};
use reth_primitives_traits::SignedTransaction;
use revm_primitives::{AuthorizationList, TxEnv};
use serde::{Deserialize, Serialize};

/// Signed transaction.
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(rlp))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Deref, Serialize, Deserialize, Constructor)]
pub struct OpTransactionSigned {
    /// Transaction hash
    pub hash: TxHash,
    /// The transaction signature values
    pub signature: Signature,
    /// Raw transaction info
    #[deref]
    #[as_ref]
    pub transaction: OpTypedTransaction,
}

impl OpTransactionSigned {
    /// Calculates the signing hash for the transaction.
    pub fn signature_hash(&self) -> B256 {
        match &self.transaction {
            OpTypedTransaction::Legacy(tx) => tx.signature_hash(),
            OpTypedTransaction::Eip2930(tx) => tx.signature_hash(),
            OpTypedTransaction::Eip1559(tx) => tx.signature_hash(),
            OpTypedTransaction::Eip7702(tx) => tx.signature_hash(),
            OpTypedTransaction::Deposit(_) => unreachable!(),
        }
    }
}

impl SignedTransaction for OpTransactionSigned {
    type Transaction = OpTypedTransaction;

    fn tx_hash(&self) -> &TxHash {
        &self.hash
    }

    fn transaction(&self) -> &Self::Transaction {
        &self.transaction
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn recover_signer(&self) -> Option<Address> {
        // Optimism's Deposit transaction does not have a signature. Directly return the
        // `from` address.
        if let OpTypedTransaction::Deposit(TxDeposit { from, .. }) = self.transaction {
            return Some(from)
        }

        let signature_hash = self.signature_hash();
        recover_signer(&self.signature, signature_hash)
    }

    fn recover_signer_unchecked(&self) -> Option<Address> {
        // Optimism's Deposit transaction does not have a signature. Directly return the
        // `from` address.
        if let OpTypedTransaction::Deposit(TxDeposit { from, .. }) = self.transaction {
            return Some(from)
        }
        let signature_hash = self.signature_hash();
        recover_signer_unchecked(&self.signature, signature_hash)
    }

    fn from_transaction_and_signature(
        transaction: Self::Transaction,
        signature: Signature,
    ) -> Self {
        let mut initial_tx = Self { transaction, hash: Default::default(), signature };
        initial_tx.hash = initial_tx.recalculate_hash();
        initial_tx
    }

    fn recalculate_hash(&self) -> B256 {
        keccak256(self.encoded_2718())
    }

    fn fill_tx_env(&self, tx_env: &mut TxEnv, sender: Address) {
        let envelope = self.encoded_2718();

        tx_env.caller = sender;
        match self.as_ref() {
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

                tx_env.optimism = revm_primitives::OptimismFields {
                    source_hash: Some(tx.source_hash),
                    mint: tx.mint,
                    is_system_transaction: Some(tx.is_system_transaction),
                    enveloped_tx: Some(envelope.into()),
                };
                return
            }
        }

        tx_env.optimism = revm_primitives::OptimismFields {
            source_hash: None,
            mint: None,
            is_system_transaction: Some(false),
            enveloped_tx: Some(envelope.into()),
        }
    }
}

impl alloy_rlp::Encodable for OpTransactionSigned {
    /// See [`alloy_rlp::Encodable`] impl for [`TransactionSigned`].
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

impl alloy_rlp::Decodable for OpTransactionSigned {
    /// See [`alloy_rlp::Decodable`] impl for [`TransactionSigned`].
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Self::network_decode(buf).map_err(Into::into)
    }
}

impl Encodable2718 for OpTransactionSigned {
    fn type_flag(&self) -> Option<u8> {
        match self.transaction.tx_type() {
            OpTxType::Legacy => None,
            tx_type => Some(tx_type as u8),
        }
    }

    fn encode_2718_len(&self) -> usize {
        match &self.transaction {
            OpTypedTransaction::Legacy(legacy_tx) => legacy_tx.encoded_len_with_signature(
                &with_eip155_parity(&self.signature, legacy_tx.chain_id),
            ),
            OpTypedTransaction::Eip2930(access_list_tx) => {
                access_list_tx.encoded_len_with_signature(&self.signature, false)
            }
            OpTypedTransaction::Eip1559(dynamic_fee_tx) => {
                dynamic_fee_tx.encoded_len_with_signature(&self.signature, false)
            }
            OpTypedTransaction::Eip7702(set_code_tx) => {
                set_code_tx.encoded_len_with_signature(&self.signature, false)
            }
            OpTypedTransaction::Deposit(deposit_tx) => deposit_tx.encoded_len(false),
        }
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        let Self { transaction, signature, .. } = self;

        match transaction {
            OpTypedTransaction::Legacy(legacy_tx) => {
                // do nothing w/ with_header
                legacy_tx.encode_with_signature_fields(
                    &with_eip155_parity(signature, legacy_tx.chain_id),
                    out,
                )
            }
            OpTypedTransaction::Eip2930(access_list_tx) => {
                access_list_tx.encode_with_signature(signature, out, false)
            }
            OpTypedTransaction::Eip1559(dynamic_fee_tx) => {
                dynamic_fee_tx.encode_with_signature(signature, out, false)
            }
            OpTypedTransaction::Eip7702(set_code_tx) => {
                set_code_tx.encode_with_signature(signature, out, false)
            }
            OpTypedTransaction::Deposit(deposit_tx) => deposit_tx.encode_inner(out, false),
        }
    }
}

impl Decodable2718 for OpTransactionSigned {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        match ty.try_into().map_err(|_| Eip2718Error::UnexpectedType(ty))? {
            OpTxType::Legacy => Err(Eip2718Error::UnexpectedType(0)),
            OpTxType::Eip2930 => {
                let (tx, signature, hash) = TxEip2930::decode_signed_fields(buf)?.into_parts();
                Ok(Self::new(hash, signature, OpTypedTransaction::Eip2930(tx)))
            }
            OpTxType::Eip1559 => {
                let (tx, signature, hash) = TxEip1559::decode_signed_fields(buf)?.into_parts();
                Ok(Self::new(hash, signature, OpTypedTransaction::Eip1559(tx)))
            }
            OpTxType::Eip7702 => {
                let (tx, signature, hash) = TxEip7702::decode_signed_fields(buf)?.into_parts();
                Ok(Self::new(hash, signature, OpTypedTransaction::Eip7702(tx)))
            }
            OpTxType::Deposit => Ok(Self::from_transaction_and_signature(
                OpTypedTransaction::Deposit(TxDeposit::decode(buf)?),
                TxDeposit::signature(),
            )),
        }
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        let (transaction, hash, signature) =
            TransactionSigned::decode_rlp_legacy_transaction_tuple(buf)?;
        Ok(Self::new(hash, signature, OpTypedTransaction::Legacy(transaction)))
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for OpTransactionSigned {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        #[allow(unused_mut)]
        let mut transaction = OpTypedTransaction::arbitrary(u)?;
        let mut signature = alloy_primitives::Signature::arbitrary(u)?;

        signature = if matches!(transaction, OpTypedTransaction::Legacy(_)) {
            if let Some(chain_id) = transaction.chain_id() {
                signature.with_chain_id(chain_id)
            } else {
                signature.with_parity(alloy_primitives::Parity::NonEip155(bool::arbitrary(u)?))
            }
        } else {
            signature.with_parity_bool()
        };

        // Both `Some(0)` and `None` values are encoded as empty string byte. This introduces
        // ambiguity in roundtrip tests. Patch the mint value of deposit transaction here, so that
        // it's `None` if zero.
        if let OpTypedTransaction::Deposit(ref mut tx_deposit) = transaction {
            if tx_deposit.mint == Some(0) {
                tx_deposit.mint = None;
            }
        }

        if let OpTypedTransaction::Deposit(_) = transaction {
            signature = TxDeposit::signature()
        }

        Ok(Self::from_transaction_and_signature(transaction, signature))
    }
}
