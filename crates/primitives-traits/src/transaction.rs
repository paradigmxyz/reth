use core::fmt::Debug;

use alloy_primitives::{Bytes, TxKind, B256, U256};
use alloy_serde::WithOtherFields;

use alloy_consensus::{
    Transaction as AlloyConsensusTransaction, TxEip1559, TxEip2930, TxEip4844, TxEip7702, TxLegacy,
};
use alloy_rlp::Encodable;

#[cfg(feature = "arbitrary")]
pub trait Transaction<T>:
    Debug
    + Clone
    + PartialEq
    + Eq
    + std::hash::Hash
    + serde::Serialize
    + for<'a> serde::Deserialize<'a>
    + derive_more::From<T>
    + AlloyConsensusTransaction
    + TryFrom<WithOtherFields<alloy_rpc_types::Transaction>>
    + for<'b> arbitrary::Arbitrary<'b>
    + reth_codecs::Compact
    + Default
    + Encodable
{
    fn signature_hash(&self) -> B256;
    fn set_chain_id(&mut self, chain_id: u64);
    fn kind(&self) -> TxKind;
    fn is_dynamic_fee(&self) -> bool;
    fn blob_gas_used(&self) -> Option<u64>;
    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128;
    #[cfg(feature = "optimism")]
    fn source_hash(&self) -> Option<B256>;
    #[cfg(feature = "optimism")]
    fn mint(&self) -> Option<u128>;
    #[cfg(feature = "optimism")]
    fn is_system_transaction(&self) -> bool;
    #[cfg(feature = "optimism")]
    fn is_deposit(&self) -> bool;
    fn encode_without_signature(&self, out: &mut dyn bytes::BufMut);
    fn set_gas_limit(&mut self, gas_limit: u64);
    fn set_nonce(&mut self, nonce: u64);
    fn set_value(&mut self, value: U256);
    fn set_input(&mut self, input: Bytes);
    fn size(&self) -> usize;
    fn is_legacy(&self) -> bool;
    fn is_eip2930(&self) -> bool;
    fn is_eip1559(&self) -> bool;
    fn is_eip4844(&self) -> bool;
    fn is_eip7702(&self) -> bool;
    fn as_legacy(&self) -> Option<&TxLegacy>;
    fn as_eip2930(&self) -> Option<&TxEip2930>;
    fn as_eip1559(&self) -> Option<&TxEip1559>;
    fn as_eip4844(&self) -> Option<&TxEip4844>;
    fn as_eip7702(&self) -> Option<&TxEip7702>;
}
