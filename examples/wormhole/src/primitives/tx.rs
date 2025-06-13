use alloy_consensus::Transaction;
use alloy_eips::eip2718::{Decodable2718, Eip2718Error, Encodable2718, Typed2718};
use alloy_primitives::{Address, Bytes, TxHash, TxKind};
use alloy_rlp::{Decodable, Encodable};
use reth_op::primitives::SignedTransaction;
// Note: For this demo, we're not actually using the real WormholeTx
// In a production version, you'd implement proper wormhole transaction support
use reth_op::OpTransactionSigned;
use std::fmt;

/// Wormhole transaction type ID
pub const WORMHOLE_TX_TYPE_ID: u8 = 0x7E; // 126

/// Extended transaction envelope that includes both Optimism and Wormhole transactions
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum WormholeTransaction {
    /// Standard Optimism transaction
    Optimism(OpTransactionSigned),
    /// Wormhole transaction (simplified for now - only handles OP transactions in practice)
    Wormhole(OpTransactionSigned), // Using OpTransactionSigned instead of WormholeTx for now
}

impl WormholeTransaction {
    /// Returns the transaction type
    pub fn tx_type(&self) -> u8 {
        match self {
            Self::Optimism(tx) => tx.tx_type() as u8,
            Self::Wormhole(_) => WORMHOLE_TX_TYPE_ID,
        }
    }

    /// Returns true if this is a wormhole transaction
    pub fn is_wormhole(&self) -> bool {
        matches!(self, Self::Wormhole(_))
    }

    /// Returns the transaction hash
    pub fn hash(&self) -> TxHash {
        let encoded = self.encoded_2718();
        TxHash::from(alloy_primitives::keccak256(&encoded))
    }
}

impl From<OpTransactionSigned> for WormholeTransaction {
    fn from(tx: OpTransactionSigned) -> Self {
        Self::Optimism(tx)
    }
}

// impl From<WormholeTx> removed since we're using OpTransactionSigned for demo

impl Typed2718 for WormholeTransaction {
    fn ty(&self) -> u8 {
        self.tx_type()
    }
}

impl Transaction for WormholeTransaction {
    // Required methods in order from the trait definition
    fn chain_id(&self) -> Option<u64> {
        match self {
            Self::Optimism(tx) => tx.chain_id(),
            Self::Wormhole(tx) => tx.chain_id(),
        }
    }

    fn nonce(&self) -> u64 {
        match self {
            Self::Optimism(tx) => tx.nonce(),
            Self::Wormhole(tx) => tx.nonce(),
        }
    }

    fn gas_limit(&self) -> u64 {
        match self {
            Self::Optimism(tx) => tx.gas_limit(),
            Self::Wormhole(tx) => tx.gas_limit(),
        }
    }

    fn gas_price(&self) -> Option<u128> {
        match self {
            Self::Optimism(tx) => tx.gas_price(),
            Self::Wormhole(tx) => tx.gas_price(),
        }
    }

    fn max_fee_per_gas(&self) -> u128 {
        match self {
            Self::Optimism(tx) => tx.max_fee_per_gas(),
            Self::Wormhole(tx) => tx.max_fee_per_gas(),
        }
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        match self {
            Self::Optimism(tx) => tx.max_priority_fee_per_gas(),
            Self::Wormhole(tx) => tx.max_priority_fee_per_gas(),
        }
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        match self {
            Self::Optimism(tx) => tx.max_fee_per_blob_gas(),
            Self::Wormhole(_) => None,
        }
    }

    fn priority_fee_or_price(&self) -> u128 {
        match self {
            Self::Optimism(tx) => tx.priority_fee_or_price(),
            Self::Wormhole(tx) => tx.priority_fee_or_price(),
        }
    }

    fn kind(&self) -> TxKind {
        match self {
            Self::Optimism(tx) => tx.kind(),
            Self::Wormhole(tx) => tx.kind(),
        }
    }

    fn value(&self) -> alloy_primitives::U256 {
        match self {
            Self::Optimism(tx) => tx.value(),
            Self::Wormhole(tx) => tx.value(),
        }
    }

    fn input(&self) -> &Bytes {
        match self {
            Self::Optimism(tx) => tx.input(),
            Self::Wormhole(tx) => tx.input(),
        }
    }

    fn access_list(&self) -> Option<&alloy_eips::eip2930::AccessList> {
        match self {
            Self::Optimism(tx) => tx.access_list(),
            Self::Wormhole(tx) => tx.access_list(),
        }
    }

    fn blob_versioned_hashes(&self) -> Option<&[alloy_primitives::B256]> {
        match self {
            Self::Optimism(tx) => tx.blob_versioned_hashes(),
            Self::Wormhole(_) => None,
        }
    }

    fn authorization_list(&self) -> Option<&[alloy_eips::eip7702::SignedAuthorization]> {
        match self {
            Self::Optimism(tx) => tx.authorization_list(),
            Self::Wormhole(_) => None,
        }
    }

    fn is_dynamic_fee(&self) -> bool {
        match self {
            Self::Optimism(tx) => tx.is_dynamic_fee(),
            Self::Wormhole(tx) => tx.is_dynamic_fee(),
        }
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        match self {
            Self::Optimism(tx) => tx.effective_gas_price(base_fee),
            Self::Wormhole(tx) => tx.effective_gas_price(base_fee),
        }
    }

    fn is_create(&self) -> bool {
        match self {
            Self::Optimism(tx) => tx.is_create(),
            Self::Wormhole(tx) => tx.is_create(),
        }
    }

    fn to(&self) -> Option<Address> {
        match self {
            Self::Optimism(tx) => tx.to(),
            Self::Wormhole(tx) => tx.to(),
        }
    }
}

impl Encodable2718 for WormholeTransaction {
    fn type_flag(&self) -> Option<u8> {
        Some(self.tx_type())
    }

    fn encode_2718_len(&self) -> usize {
        match self {
            Self::Optimism(tx) => tx.encode_2718_len(),
            Self::Wormhole(tx) => tx.encode_2718_len(), // Same as optimism for demo
        }
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        match self {
            Self::Optimism(tx) => tx.encode_2718(out),
            Self::Wormhole(tx) => {
                // For demo purposes, encode with wormhole type prefix
                out.put_u8(WORMHOLE_TX_TYPE_ID);
                // Then encode the inner OP transaction without its type prefix
                tx.encode(out);
            }
        }
    }
}

impl Decodable2718 for WormholeTransaction {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Result<Self, Eip2718Error> {
        if ty == WORMHOLE_TX_TYPE_ID {
            // For demo, decode as OP transaction but mark as wormhole
            Ok(Self::Wormhole(OpTransactionSigned::decode(buf).map_err(Eip2718Error::RlpError)?))
        } else {
            Ok(Self::Optimism(OpTransactionSigned::typed_decode(ty, buf)?))
        }
    }

    fn fallback_decode(buf: &mut &[u8]) -> Result<Self, Eip2718Error> {
        Ok(Self::Optimism(OpTransactionSigned::fallback_decode(buf)?))
    }
}

impl fmt::Display for WormholeTransaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Optimism(tx) => write!(f, "Optimism({:?})", tx),
            Self::Wormhole(_) => write!(f, "Wormhole"),
        }
    }
}

// We need a signed variant that stores the hash
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct WormholeTransactionSigned {
    transaction: WormholeTransaction,
    hash: TxHash,
}

impl WormholeTransactionSigned {
    /// Create a new signed transaction with computed hash
    pub fn new(transaction: WormholeTransaction) -> Self {
        let hash = transaction.hash();
        Self { transaction, hash }
    }

    /// Get the inner transaction
    pub fn transaction(&self) -> &WormholeTransaction {
        &self.transaction
    }

    /// Create from components
    pub fn from_parts(transaction: WormholeTransaction, hash: TxHash) -> Self {
        Self { transaction, hash }
    }
}

impl SignedTransaction for WormholeTransactionSigned {
    fn tx_hash(&self) -> &TxHash {
        &self.hash
    }

    fn recover_signer_unchecked_with_buf(
        &self,
        buf: &mut Vec<u8>,
    ) -> Result<Address, reth_primitives_traits::crypto::RecoveryError> {
        match &self.transaction {
            WormholeTransaction::Optimism(tx) => tx.recover_signer_unchecked_with_buf(buf),
            WormholeTransaction::Wormhole(tx) => {
                // Since we're using OpTransactionSigned for demo, delegate to it
                tx.recover_signer_unchecked_with_buf(buf)
            }
        }
    }
}

// Forward Transaction trait implementation
impl Transaction for WormholeTransactionSigned {
    fn chain_id(&self) -> Option<u64> {
        self.transaction.chain_id()
    }
    fn nonce(&self) -> u64 {
        self.transaction.nonce()
    }
    fn gas_limit(&self) -> u64 {
        self.transaction.gas_limit()
    }
    fn gas_price(&self) -> Option<u128> {
        self.transaction.gas_price()
    }
    fn max_fee_per_gas(&self) -> u128 {
        self.transaction.max_fee_per_gas()
    }
    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.transaction.max_priority_fee_per_gas()
    }
    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        self.transaction.max_fee_per_blob_gas()
    }
    fn priority_fee_or_price(&self) -> u128 {
        self.transaction.priority_fee_or_price()
    }
    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        self.transaction.effective_gas_price(base_fee)
    }
    fn is_dynamic_fee(&self) -> bool {
        self.transaction.is_dynamic_fee()
    }
    fn kind(&self) -> TxKind {
        self.transaction.kind()
    }
    fn is_create(&self) -> bool {
        self.transaction.is_create()
    }
    fn to(&self) -> Option<Address> {
        self.transaction.to()
    }
    fn value(&self) -> alloy_primitives::U256 {
        self.transaction.value()
    }
    fn input(&self) -> &Bytes {
        self.transaction.input()
    }
    fn access_list(&self) -> Option<&alloy_eips::eip2930::AccessList> {
        self.transaction.access_list()
    }
    fn blob_versioned_hashes(&self) -> Option<&[alloy_primitives::B256]> {
        self.transaction.blob_versioned_hashes()
    }
    fn authorization_list(&self) -> Option<&[alloy_eips::eip7702::SignedAuthorization]> {
        self.transaction.authorization_list()
    }
}

// Forward other trait implementations
impl Typed2718 for WormholeTransactionSigned {
    fn ty(&self) -> u8 {
        self.transaction.ty()
    }
}

impl Encodable2718 for WormholeTransactionSigned {
    fn type_flag(&self) -> Option<u8> {
        self.transaction.type_flag()
    }
    fn encode_2718_len(&self) -> usize {
        self.transaction.encode_2718_len()
    }
    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.transaction.encode_2718(out)
    }
}

impl Decodable2718 for WormholeTransactionSigned {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Result<Self, Eip2718Error> {
        Ok(Self::new(WormholeTransaction::typed_decode(ty, buf)?))
    }
    fn fallback_decode(buf: &mut &[u8]) -> Result<Self, Eip2718Error> {
        Ok(Self::new(WormholeTransaction::fallback_decode(buf)?))
    }
}

impl Encodable for WormholeTransactionSigned {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.transaction.encode(out)
    }
    fn length(&self) -> usize {
        self.transaction.length()
    }
}

impl Decodable for WormholeTransactionSigned {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self::new(WormholeTransaction::decode(buf)?))
    }
}

impl alloy_consensus::transaction::SignerRecoverable for WormholeTransactionSigned {
    fn recover_signer(&self) -> Result<Address, alloy_consensus::crypto::RecoveryError> {
        self.transaction.recover_signer()
    }
    fn recover_signer_unchecked(&self) -> Result<Address, alloy_consensus::crypto::RecoveryError> {
        self.transaction.recover_signer_unchecked()
    }
}

impl reth_codecs::Compact for WormholeTransactionSigned {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        self.transaction.to_compact(buf)
    }
    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (tx, rest) = WormholeTransaction::from_compact(buf, len);
        (Self::new(tx), rest)
    }
}

impl reth_op::primitives::InMemorySize for WormholeTransactionSigned {
    fn size(&self) -> usize {
        self.transaction.size() + std::mem::size_of::<TxHash>()
    }
}

impl fmt::Display for WormholeTransactionSigned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.transaction)
    }
}

impl reth_primitives_traits::serde_bincode_compat::SerdeBincodeCompat
    for WormholeTransactionSigned
{
    // Use Bytes as the bincode representation
    type BincodeRepr<'a> = alloy_primitives::Bytes;

    fn as_repr(&self) -> Self::BincodeRepr<'_> {
        // Encode the entire signed transaction as RLP
        let mut buf = Vec::new();
        self.encode(&mut buf);
        alloy_primitives::Bytes::from(buf)
    }

    fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
        // Decode from RLP bytes
        Self::decode(&mut repr.as_ref()).expect("valid RLP encoded signed transaction")
    }
}

// Implement SignerRecoverable trait
impl alloy_consensus::transaction::SignerRecoverable for WormholeTransaction {
    fn recover_signer(&self) -> Result<Address, alloy_consensus::crypto::RecoveryError> {
        match self {
            Self::Optimism(tx) => tx.recover_signer(),
            Self::Wormhole(tx) => {
                // Since we're using OpTransactionSigned for demo, delegate to it
                tx.recover_signer()
            }
        }
    }

    fn recover_signer_unchecked(&self) -> Result<Address, alloy_consensus::crypto::RecoveryError> {
        match self {
            Self::Optimism(tx) => tx.recover_signer_unchecked(),
            Self::Wormhole(tx) => {
                // Since we're using OpTransactionSigned for demo, delegate to it
                tx.recover_signer_unchecked()
            }
        }
    }
}

impl Encodable for WormholeTransaction {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        // Use EIP-2718 encoding
        self.encode_2718(out);
    }

    fn length(&self) -> usize {
        self.encode_2718_len()
    }
}

impl Decodable for WormholeTransaction {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        // Use EIP-2718 decoding
        Self::decode_2718(buf)
            .map_err(|_| alloy_rlp::Error::Custom("Failed to decode WormholeTransaction"))
    }
}

/// Implement Compact for WormholeTransaction
/// This is a placeholder implementation - in production you'd implement proper compression
impl reth_codecs::Compact for WormholeTransaction {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        // For now, just encode the full transaction
        let encoded = self.encoded_2718();
        buf.put_slice(&encoded);
        encoded.len()
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
        // For now, just decode from the buffer
        let tx = Self::decode_2718(&mut &buf[..]).expect("valid transaction");
        (tx, &[])
    }
}

/// Implement InMemorySize for WormholeTransaction
impl reth_op::primitives::InMemorySize for WormholeTransaction {
    fn size(&self) -> usize {
        // Return the encoded size as an approximation
        self.encode_2718_len()
    }
}

// Module for SerdeBincodeCompat implementation
mod serde_bincode_compat {
    // Empty module - we'll implement SerdeBincodeCompat directly
} // End of serde_bincode_compat module

// Note: Using derive for serde traits for simplicity
// In production you might want custom serialization for specific formats

impl reth_primitives_traits::serde_bincode_compat::SerdeBincodeCompat for WormholeTransaction {
    // Use raw bytes as the bincode representation
    type BincodeRepr<'a> = std::borrow::Cow<'a, [u8]>;

    fn as_repr(&self) -> Self::BincodeRepr<'_> {
        // Encode as RLP 2718 bytes
        std::borrow::Cow::Owned(self.encoded_2718().to_vec())
    }

    fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
        // Decode from RLP 2718 bytes
        Self::decode_2718(&mut repr.as_ref()).expect("valid RLP 2718 encoded transaction")
    }
}
