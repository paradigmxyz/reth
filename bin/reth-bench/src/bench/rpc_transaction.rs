use alloy_consensus::Transaction;
use alloy_eips::{eip7702::SignedAuthorization, Encodable2718, Typed2718};
use alloy_primitives::{bytes::BufMut, Bytes, ChainId, TxKind, B256, U256};
use alloy_rpc_types::{AccessList, Transaction as EthRpcTransaction};
use delegate::delegate;
use op_alloy_rpc_types::Transaction as OpRpcTransaction;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum RpcTransaction {
    Ethereum(EthRpcTransaction),
    Optimism(OpRpcTransaction),
}

impl Typed2718 for RpcTransaction {
    delegate! {
        to match self {
            Self::Ethereum(tx) => tx,
            Self::Optimism(tx) => tx,
        } {
            fn ty(&self) -> u8;
        }
    }
}

impl Transaction for RpcTransaction {
    delegate! {
        to match self {
            Self::Ethereum(tx) => tx,
            Self::Optimism(tx) => tx,
        } {
            fn chain_id(&self) -> Option<ChainId>;
            fn nonce(&self) -> u64;
            fn gas_limit(&self) -> u64;
            fn gas_price(&self) -> Option<u128>;
            fn max_fee_per_gas(&self) -> u128;
            fn max_priority_fee_per_gas(&self) -> Option<u128>;
            fn max_fee_per_blob_gas(&self) -> Option<u128>;
            fn priority_fee_or_price(&self) -> u128;
            fn effective_gas_price(&self, base_fee: Option<u64>) -> u128;
            fn is_dynamic_fee(&self) -> bool;
            fn kind(&self) -> TxKind;
            fn is_create(&self) -> bool;
            fn value(&self) -> U256;
            fn input(&self) -> &Bytes;
            fn access_list(&self) -> Option<&AccessList>;
            fn blob_versioned_hashes(&self) -> Option<&[B256]>;
            fn authorization_list(&self) -> Option<&[SignedAuthorization]>;
        }
    }
}

impl Encodable2718 for RpcTransaction {
    delegate! {
        to match self {
            Self::Ethereum(tx) => tx.inner,
            Self::Optimism(tx) => tx.inner.inner,
        } {
            fn encode_2718_len(&self) -> usize;
            fn encode_2718(&self, out: &mut dyn BufMut);
        }
    }
}
