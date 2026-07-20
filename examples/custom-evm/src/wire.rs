//! Wire-level adapter for the custom evm2 transaction.
//!
//! The node's consensus transaction stays signed and encodable, while the EVM registry consumes
//! the small `CustomEnvelope` used by the execution handler. A real custom node uses this adapter
//! from its pool/network and engine conversion layers.

use crate::tx::{CustomEnvelope, ExecuteCodeTx};
use alloy_consensus::{
    transaction::{
        Recovered, RlpEcdsaDecodableTx, RlpEcdsaEncodableTx, SignableTransaction, SignerRecoverable,
    },
    Transaction,
};
use alloy_eips::eip2718::Typed2718;
use alloy_primitives::{Address, Bytes, ChainId, Signature, TxKind, B256, U256};
use alloy_rlp::{BufMut, Decodable, Encodable};
use reth_ethereum::primitives::InMemorySize;
use std::mem;

/// EIP-2718 type byte for the wire-level custom transaction.
pub const CUSTOM_WIRE_TX_TYPE: u8 = 0x7f;

/// Signed custom transaction payload.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct WireTx {
    /// Replay-protection chain ID.
    pub chain_id: ChainId,
    /// Sender nonce.
    pub nonce: u64,
    /// Gas limit.
    pub gas_limit: u64,
    /// Maximum fee per gas.
    pub max_fee_per_gas: u128,
    /// Maximum priority fee per gas.
    pub max_priority_fee_per_gas: u128,
    /// Message target.
    pub target: Address,
    /// Bytecode executed by the custom handler.
    pub code: Bytes,
}

impl WireTx {
    /// Returns the custom transaction type.
    pub const fn tx_type() -> u8 {
        CUSTOM_WIRE_TX_TYPE
    }
}

impl RlpEcdsaEncodableTx for WireTx {
    fn rlp_encoded_fields_length(&self) -> usize {
        self.chain_id.length() +
            self.nonce.length() +
            self.max_priority_fee_per_gas.length() +
            self.max_fee_per_gas.length() +
            self.gas_limit.length() +
            self.target.length() +
            self.code.length()
    }

    fn rlp_encode_fields(&self, out: &mut dyn BufMut) {
        self.chain_id.encode(out);
        self.nonce.encode(out);
        self.max_priority_fee_per_gas.encode(out);
        self.max_fee_per_gas.encode(out);
        self.gas_limit.encode(out);
        self.target.encode(out);
        self.code.encode(out);
    }
}

impl RlpEcdsaDecodableTx for WireTx {
    const DEFAULT_TX_TYPE: u8 = CUSTOM_WIRE_TX_TYPE;

    fn rlp_decode_fields(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self {
            chain_id: Decodable::decode(buf)?,
            nonce: Decodable::decode(buf)?,
            max_priority_fee_per_gas: Decodable::decode(buf)?,
            max_fee_per_gas: Decodable::decode(buf)?,
            gas_limit: Decodable::decode(buf)?,
            target: Decodable::decode(buf)?,
            code: Decodable::decode(buf)?,
        })
    }
}

impl Transaction for WireTx {
    fn chain_id(&self) -> Option<ChainId> {
        Some(self.chain_id)
    }

    fn nonce(&self) -> u64 {
        self.nonce
    }

    fn gas_limit(&self) -> u64 {
        self.gas_limit
    }

    fn gas_price(&self) -> Option<u128> {
        None
    }

    fn max_fee_per_gas(&self) -> u128 {
        self.max_fee_per_gas
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        Some(self.max_priority_fee_per_gas)
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        None
    }

    fn priority_fee_or_price(&self) -> u128 {
        self.max_priority_fee_per_gas
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        base_fee.map_or(self.max_fee_per_gas, |base_fee| {
            self.max_fee_per_gas.min(base_fee as u128 + self.max_priority_fee_per_gas)
        })
    }

    fn is_dynamic_fee(&self) -> bool {
        true
    }

    fn kind(&self) -> TxKind {
        TxKind::Call(self.target)
    }

    fn is_create(&self) -> bool {
        false
    }

    fn value(&self) -> U256 {
        U256::ZERO
    }

    fn input(&self) -> &Bytes {
        &self.code
    }

    fn access_list(&self) -> Option<&alloy_eips::eip2930::AccessList> {
        None
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        None
    }

    fn authorization_list(&self) -> Option<&[alloy_eips::eip7702::SignedAuthorization]> {
        None
    }
}

impl Typed2718 for WireTx {
    fn ty(&self) -> u8 {
        CUSTOM_WIRE_TX_TYPE
    }
}

impl SignableTransaction<Signature> for WireTx {
    fn set_chain_id(&mut self, chain_id: ChainId) {
        self.chain_id = chain_id;
    }

    fn encode_for_signing(&self, out: &mut dyn BufMut) {
        out.put_u8(CUSTOM_WIRE_TX_TYPE);
        self.encode(out);
    }

    fn payload_len_for_signature(&self) -> usize {
        self.length() + 1
    }
}

impl Encodable for WireTx {
    fn encode(&self, out: &mut dyn BufMut) {
        self.rlp_encode(out);
    }

    fn length(&self) -> usize {
        self.rlp_encoded_length()
    }
}

impl Decodable for WireTx {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Self::rlp_decode(buf)
    }
}

impl InMemorySize for WireTx {
    fn size(&self) -> usize {
        mem::size_of::<Self>() + self.code.len()
    }
}

/// Signed wire-level custom transaction.
pub type SignedCustomTransaction = alloy_consensus::Signed<WireTx>;

/// Recovers the sender from a signed wire transaction.
pub fn recover_wire_transaction(
    transaction: SignedCustomTransaction,
) -> Result<Recovered<SignedCustomTransaction>, alloy_consensus::crypto::RecoveryError> {
    <SignedCustomTransaction as SignerRecoverable>::try_into_recovered(transaction)
}

/// Converts a recovered wire transaction into the evm2 transaction registry envelope.
pub fn into_evm_transaction(transaction: Recovered<SignedCustomTransaction>) -> CustomEnvelope {
    let (transaction, caller) = transaction.into_parts();
    let tx = transaction.strip_signature();
    CustomEnvelope::ExecuteCode(ExecuteCodeTx {
        caller,
        target: tx.target,
        code: tx.code,
        gas_limit: tx.gas_limit,
    })
}
