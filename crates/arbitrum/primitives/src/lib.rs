#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

use alloc::vec::Vec;
use alloc::{fmt::Debug, sync::Arc};
use alloy_consensus::Receipt as AlloyReceipt;
use alloy_consensus::{Sealed, SignableTransaction, Signed, Transaction as ConsensusTx, TxLegacy, Typed2718};
use alloy_eips::eip2718::{Decodable2718, Eip2718Error, Eip2718Result, Encodable2718};
use alloy_primitives::{keccak256, Address, Bytes, Signature, TxHash, TxKind, U256, B256};
use alloy_rlp::{Decodable, Encodable};
use core::hash::{Hash, Hasher};
use core::ops::Deref;
use reth_primitives_traits::{InMemorySize, MaybeCompact, MaybeSerde, MaybeSerdeBincodeCompat, SignedTransaction};
use reth_primitives_traits::crypto::secp256k1::{recover_signer, recover_signer_unchecked};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArbReceipt {
    Legacy(AlloyReceipt),
    Eip1559(AlloyReceipt),
    Eip2930(AlloyReceipt),
    Eip7702(AlloyReceipt),
    Deposit(ArbDepositReceipt),
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ArbDepositReceipt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArbTypedTransaction {
    Deposit(arb_alloy_consensus::tx::ArbDepositTx),
    Unsigned(arb_alloy_consensus::tx::ArbUnsignedTx),
    Contract(arb_alloy_consensus::tx::ArbContractTx),
    Retry(arb_alloy_consensus::tx::ArbRetryTx),
    SubmitRetryable(arb_alloy_consensus::tx::ArbSubmitRetryableTx),
    Internal(arb_alloy_consensus::tx::ArbInternalTx),
    Legacy(TxLegacy),
}

#[derive(Clone, Debug, Eq)]
pub struct ArbTransactionSigned {
    hash: reth_primitives_traits::sync::OnceLock<TxHash>,
    signature: Signature,
    transaction: ArbTypedTransaction,
    input_cache: reth_primitives_traits::sync::OnceLock<Bytes>,
}

impl Deref for ArbTransactionSigned {
    type Target = ArbTypedTransaction;
    fn deref(&self) -> &Self::Target {
        &self.transaction
    }
}

impl ArbTransactionSigned {
    pub fn new(transaction: ArbTypedTransaction, signature: Signature, hash: B256) -> Self {
        Self { hash: hash.into(), signature, transaction }
    }

    pub fn new_unhashed(transaction: ArbTypedTransaction, signature: Signature) -> Self {
        Self { hash: Default::default(), signature, transaction }
    }

    pub const fn tx_type(&self) -> ArbTxType {
        match &self.transaction {
            ArbTypedTransaction::Deposit(_) => ArbTxType::Deposit,
            ArbTypedTransaction::Unsigned(_) => ArbTxType::Unsigned,
            ArbTypedTransaction::Contract(_) => ArbTxType::Contract,
            ArbTypedTransaction::Retry(_) => ArbTxType::Retry,
            ArbTypedTransaction::SubmitRetryable(_) => ArbTxType::SubmitRetryable,
            ArbTypedTransaction::Internal(_) => ArbTxType::Internal,
            ArbTypedTransaction::Legacy(_) => ArbTxType::Legacy,
        }
    }

    pub fn split(self) -> (ArbTypedTransaction, Signature, B256) {
        let hash = *self.hash.get_or_init(|| self.recalculate_hash());
        (self.transaction, self.signature, hash)
    }
}

impl reth_primitives_traits::transaction::signed::SignerRecoverable for ArbTransactionSigned {
    fn recover_signer(&self) -> Result<Address, reth_primitives_traits::transaction::signed::RecoveryError> {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => {
                let sig_hash = alloy_consensus::transaction::signature_hash(tx);
                recover_signer(&self.signature, sig_hash)
            }
            ArbTypedTransaction::Deposit(tx) => Ok(tx.from),
            ArbTypedTransaction::Unsigned(tx) => Ok(tx.from),
            ArbTypedTransaction::Contract(tx) => Ok(tx.from),
            ArbTypedTransaction::Retry(tx) => Ok(tx.from),
            ArbTypedTransaction::SubmitRetryable(tx) => Ok(tx.from),
            ArbTypedTransaction::Internal(_) => Ok(Address::ZERO),
        }
    }

    fn recover_signer_unchecked(&self) -> Result<Address, reth_primitives_traits::transaction::signed::RecoveryError> {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => {
                let sig_hash = alloy_consensus::transaction::signature_hash(tx);
                recover_signer_unchecked(&self.signature, sig_hash)
            }
            ArbTypedTransaction::Deposit(tx) => Ok(tx.from),
            ArbTypedTransaction::Unsigned(tx) => Ok(tx.from),
            ArbTypedTransaction::Contract(tx) => Ok(tx.from),
            ArbTypedTransaction::Retry(tx) => Ok(tx.from),
            ArbTypedTransaction::SubmitRetryable(tx) => Ok(tx.from),
            ArbTypedTransaction::Internal(_) => Ok(Address::ZERO),
        }
    }
}

impl SignedTransaction for ArbTransactionSigned {
    fn tx_hash(&self) -> &TxHash {
        self.hash.get_or_init(|| self.recalculate_hash())
    }
}

impl Hash for ArbTransactionSigned {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.tx_hash().hash(state)
    }
}

impl PartialEq for ArbTransactionSigned {
    fn eq(&self, other: &Self) -> bool {
        self.tx_hash() == other.tx_hash()
    }
}

impl InMemorySize for ArbTransactionSigned {
    fn size(&self) -> usize {
        core::mem::size_of::<TxHash>() + core::mem::size_of::<Signature>()
    }
}

impl MaybeSerde for ArbTransactionSigned {}
impl MaybeCompact for ArbTransactionSigned {}
impl MaybeSerdeBincodeCompat for ArbTransactionSigned {}

impl Encodable for ArbTransactionSigned {
    fn encode(&self, out: &mut dyn alloy_rlp::bytes::BufMut) {
        self.network_encode(out);
    }
    fn length(&self) -> usize {
        let mut payload_length = self.encode_2718_len();
        if !Typed2718::is_legacy(self) {
            payload_length += alloy_rlp::Header { list: false, payload_length }.length();
        }
        payload_length
    }
}

impl Decodable for ArbTransactionSigned {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Self::network_decode(buf).map_err(Into::into)
    }
}

impl Encodable2718 for ArbTransactionSigned {
    fn type_flag(&self) -> Option<u8> {
        match &self.transaction {
            ArbTypedTransaction::Legacy(_) => None,
            _ => Some(match &self.transaction {
                ArbTypedTransaction::Deposit(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumDepositTx.as_u8(),
                ArbTypedTransaction::Unsigned(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumUnsignedTx.as_u8(),
                ArbTypedTransaction::Contract(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumContractTx.as_u8(),
                ArbTypedTransaction::Retry(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumRetryTx.as_u8(),
                ArbTypedTransaction::SubmitRetryable(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumSubmitRetryableTx.as_u8(),
                ArbTypedTransaction::Internal(_) => arb_alloy_consensus::tx::ArbTxType::ArbitrumInternalTx.as_u8(),
                ArbTypedTransaction::Legacy(_) => 0,
            }),
        }
    }

    fn encode_2718_len(&self) -> usize {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => tx.eip2718_encoded_length(&self.signature),
            ArbTypedTransaction::Deposit(tx) => tx.length() + 1,
            ArbTypedTransaction::Unsigned(tx) => tx.length() + 1,
            ArbTypedTransaction::Contract(tx) => tx.length() + 1,
            ArbTypedTransaction::Retry(tx) => tx.length() + 1,
            ArbTypedTransaction::SubmitRetryable(tx) => tx.length() + 1,
            ArbTypedTransaction::Internal(tx) => tx.length() + 1,
        }
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => {
                tx.eip2718_encode(&self.signature, out)
            }
            ArbTypedTransaction::Deposit(tx) => {
                out.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumDepositTx.as_u8());
                tx.encode(out);
            }
            ArbTypedTransaction::Unsigned(tx) => {
                out.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumUnsignedTx.as_u8());
                tx.encode(out);
            }
            ArbTypedTransaction::Contract(tx) => {
                out.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumContractTx.as_u8());
                tx.encode(out);
            }
            ArbTypedTransaction::Retry(tx) => {
                out.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumRetryTx.as_u8());
                tx.encode(out);
            }
            ArbTypedTransaction::SubmitRetryable(tx) => {
                out.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumSubmitRetryableTx.as_u8());
                tx.encode(out);
            }
            ArbTypedTransaction::Internal(tx) => {
                out.put_u8(arb_alloy_consensus::tx::ArbTxType::ArbitrumInternalTx.as_u8());
                tx.encode(out);
            }
        }
    }
}

impl Decodable2718 for ArbTransactionSigned {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        match arb_alloy_consensus::tx::ArbTxType::from_u8(ty).map_err(|_| Eip2718Error::UnexpectedType(ty))? {
            arb_alloy_consensus::tx::ArbTxType::ArbitrumDepositTx => {
                let tx = arb_alloy_consensus::tx::ArbDepositTx::decode(buf)?;
                Ok(Self::new_unhashed(ArbTypedTransaction::Deposit(tx), Signature::ZERO))
            }
            arb_alloy_consensus::tx::ArbTxType::ArbitrumUnsignedTx => {
                let tx = arb_alloy_consensus::tx::ArbUnsignedTx::decode(buf)?;
                Ok(Self::new_unhashed(ArbTypedTransaction::Unsigned(tx), Signature::ZERO))
            }
            arb_alloy_consensus::tx::ArbTxType::ArbitrumContractTx => {
                let tx = arb_alloy_consensus::tx::ArbContractTx::decode(buf)?;
                Ok(Self::new_unhashed(ArbTypedTransaction::Contract(tx), Signature::ZERO))
            }
            arb_alloy_consensus::tx::ArbTxType::ArbitrumRetryTx => {
                let tx = arb_alloy_consensus::tx::ArbRetryTx::decode(buf)?;
                Ok(Self::new_unhashed(ArbTypedTransaction::Retry(tx), Signature::ZERO))
            }
            arb_alloy_consensus::tx::ArbTxType::ArbitrumSubmitRetryableTx => {
                let tx = arb_alloy_consensus::tx::ArbSubmitRetryableTx::decode(buf)?;
                Ok(Self::new_unhashed(ArbTypedTransaction::SubmitRetryable(tx), Signature::ZERO))
            }
            arb_alloy_consensus::tx::ArbTxType::ArbitrumInternalTx => {
                let tx = arb_alloy_consensus::tx::ArbInternalTx::decode(buf)?;
                Ok(Self::new_unhashed(ArbTypedTransaction::Internal(tx), Signature::ZERO))
            }
            arb_alloy_consensus::tx::ArbTxType::ArbitrumLegacyTx => Err(Eip2718Error::UnexpectedType(0x78)),
        }
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        let (tx, signature) = TxLegacy::rlp_decode_with_signature(buf)?;
        Ok(Self::new_unhashed(ArbTypedTransaction::Legacy(tx), signature))
    }
}

impl ConsensusTx for ArbTransactionSigned {
    fn chain_id(&self) -> Option<u64> {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => tx.chain_id.map(|id| id as u64),
            ArbTypedTransaction::Deposit(tx) => Some(tx.chain_id.to::<u64>()),
            ArbTypedTransaction::Unsigned(tx) => Some(tx.chain_id.to::<u64>()),
            ArbTypedTransaction::Contract(tx) => Some(tx.chain_id.to::<u64>()),
            ArbTypedTransaction::Retry(tx) => Some(tx.chain_id.to::<u64>()),
            ArbTypedTransaction::SubmitRetryable(tx) => Some(tx.chain_id.to::<u64>()),
            ArbTypedTransaction::Internal(tx) => Some(tx.chain_id.to::<u64>()),
        }
    }

    fn nonce(&self) -> u64 {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => tx.nonce,
            ArbTypedTransaction::Deposit(_) => 0,
            ArbTypedTransaction::Unsigned(tx) => tx.nonce,
            ArbTypedTransaction::Contract(_) => 0,
            ArbTypedTransaction::Retry(tx) => tx.nonce,
            ArbTypedTransaction::SubmitRetryable(_) => 0,
            ArbTypedTransaction::Internal(_) => 0,
        }
    }

    fn gas_limit(&self) -> u64 {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => tx.gas_limit,
            ArbTypedTransaction::Deposit(_) => 0,
            ArbTypedTransaction::Unsigned(tx) => tx.gas,
            ArbTypedTransaction::Contract(tx) => tx.gas,
            ArbTypedTransaction::Retry(tx) => tx.gas,
            ArbTypedTransaction::SubmitRetryable(tx) => tx.gas,
            ArbTypedTransaction::Internal(_) => 0,
        }
    }

    fn gas_price(&self) -> Option<u128> {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => Some(tx.gas_price.to()),
            _ => None,
        }
    }

    fn max_fee_per_gas(&self) -> u128 {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => tx.gas_price.to(),
            ArbTypedTransaction::Unsigned(tx) => tx.gas_fee_cap.to(),
            ArbTypedTransaction::Contract(tx) => tx.gas_fee_cap.to(),
            ArbTypedTransaction::Retry(tx) => tx.gas_fee_cap.to(),
            ArbTypedTransaction::SubmitRetryable(tx) => tx.gas_fee_cap.to(),
            _ => 0,
        }
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        None
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        None
    }

    fn priority_fee_or_price(&self) -> u128 {
        self.gas_price().unwrap_or_else(|| self.max_fee_per_gas())
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => tx.gas_price.to(),
            _ => {
                let cap = self.max_fee_per_gas();
                match base_fee {
                    Some(b) => core::cmp::min(cap, b as u128),
                    None => cap,
                }
            }
        }
    }

    fn effective_tip_per_gas(&self, base_fee: u64) -> Option<u128> {
        Some(self.effective_gas_price(Some(base_fee)) as u128)
    }

    fn is_dynamic_fee(&self) -> bool {
        !matches!(self.transaction, ArbTypedTransaction::Legacy(_))
    }

    fn kind(&self) -> TxKind {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => match tx.to {
                Some(_) => TxKind::Call,
                None => TxKind::Create,
            },
            ArbTypedTransaction::Deposit(tx) => if tx.to == Address::ZERO { TxKind::Create } else { TxKind::Call },
            ArbTypedTransaction::Unsigned(tx) => match tx.to {
                Some(_) => TxKind::Call,
                None => TxKind::Create,
            },
            ArbTypedTransaction::Contract(tx) => match tx.to {
                Some(_) => TxKind::Call,
                None => TxKind::Create,
            },
            ArbTypedTransaction::Retry(tx) => match tx.to {
                Some(_) => TxKind::Call,
                None => TxKind::Create,
            },
            ArbTypedTransaction::SubmitRetryable(tx) => match tx.retry_to {
                Some(_) => TxKind::Call,
                None => TxKind::Create,
            },
            ArbTypedTransaction::Internal(_) => TxKind::Call,
        }
    }

    fn is_create(&self) -> bool {
        matches!(self.kind(), TxKind::Create)
    }

    fn value(&self) -> U256 {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => tx.value,
            ArbTypedTransaction::Deposit(tx) => tx.value,
            ArbTypedTransaction::Unsigned(tx) => tx.value,
            ArbTypedTransaction::Contract(tx) => tx.value,
            ArbTypedTransaction::Retry(tx) => tx.value,
            ArbTypedTransaction::SubmitRetryable(tx) => tx.retry_value,
            ArbTypedTransaction::Internal(_) => U256::ZERO,
        }
    }

    fn input(&self) -> &Bytes {
        match &self.transaction {
            ArbTypedTransaction::Legacy(tx) => &tx.input,
            ArbTypedTransaction::Deposit(_) => {
                self.input_cache.get_or_init(|| Bytes::new())
            }
            ArbTypedTransaction::Unsigned(tx) => {
                self.input_cache.get_or_init(|| Bytes::from(tx.data.clone()))
            }
            ArbTypedTransaction::Contract(tx) => {
                self.input_cache.get_or_init(|| Bytes::from(tx.data.clone()))
            }
            ArbTypedTransaction::Retry(tx) => {
                self.input_cache.get_or_init(|| Bytes::from(tx.data.clone()))
            }
            ArbTypedTransaction::SubmitRetryable(tx) => {
                self.input_cache.get_or_init(|| Bytes::from(tx.retry_data.clone()))
            }
            ArbTypedTransaction::Internal(tx) => {
                self.input_cache.get_or_init(|| Bytes::from(tx.data.clone()))
            }
        }
    }

    fn access_list(&self) -> Option<&alloy_eips::eip2930::AccessList> {
        None
    }

    fn blob_versioned_hashes(&self) -> Option<&alloc::vec::Vec<B256>> {
        None
    }

    fn authorization_list(&self) -> Option<&alloc::vec::Vec<alloy_eips::eip7702::SignedAuthorization>> {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArbTxType {
    Deposit,
    Unsigned,
    Contract,
    Retry,
    SubmitRetryable,
    Internal,
    Legacy,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArbPrimitives;

impl reth_primitives_traits::NodePrimitives for ArbPrimitives {
    type Block = alloy_consensus::Block<ArbTransactionSigned>;
    type BlockHeader = alloy_consensus::Header;
    type BlockBody = alloy_consensus::BlockBody<ArbTransactionSigned>;
    type SignedTx = ArbTransactionSigned;
    type Receipt = ArbReceipt;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Eip658Value, Receipt};
    use alloy_primitives::Log;

    #[test]
    fn arb_receipt_variants_hold_alloy_receipt() {
        let r = Receipt { status: Eip658Value::Eip658(true), cumulative_gas_used: 1, logs: Vec::<Log>::new() };
        let e = ArbReceipt::Legacy(r.clone());
        match e {
            ArbReceipt::Legacy(rr) => {
                assert!(matches!(rr.status, Eip658Value::Eip658(true)));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn arb_deposit_receipt_variant_exists() {
        let d = ArbDepositReceipt::default();
        let e = ArbReceipt::Deposit(d);
        match e {
            ArbReceipt::Deposit(_) => {}
            _ => panic!("expected deposit variant"),
        }
    }
}
