//! An abstraction over ethereum signers.

use alloy_dyn_abi::TypedData;
use alloy_primitives::{Address, Signature};
use alloy_rpc_types_eth::TransactionRequest;
use dyn_clone::DynClone;
use reth_rpc_eth_types::SignError;
use std::result;

/// Result returned by [`EthSigner`] methods.
pub type Result<T> = result::Result<T, SignError>;

/// An Ethereum Signer used via RPC.
#[async_trait::async_trait]
pub trait EthSigner<T, TxReq = TransactionRequest>: Send + Sync + DynClone {
    /// Returns the available accounts for this signer.
    fn accounts(&self) -> Vec<Address>;

    /// Returns `true` whether this signer can sign for this address
    fn is_signer_for(&self, addr: &Address) -> bool {
        self.accounts().contains(addr)
    }

    /// Returns the signature
    async fn sign(&self, address: Address, message: &[u8]) -> Result<Signature>;

    /// signs a transaction request using the given account in request
    async fn sign_transaction(&self, request: TxReq, address: &Address) -> Result<T>;

    /// Encodes and signs the typed data according EIP-712. Payload must implement Eip712 trait.
    fn sign_typed_data(&self, address: Address, payload: &TypedData) -> Result<Signature>;
}

dyn_clone::clone_trait_object!(<T, TxReq> EthSigner<T, TxReq>);

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct MockSigner;

    struct MockSignedTx;
    struct MockTxReq;

    #[async_trait::async_trait]
    impl EthSigner<MockSignedTx, MockTxReq> for MockSigner {
        fn accounts(&self) -> Vec<Address> {
            Vec::new()
        }

        async fn sign(&self, _address: Address, _message: &[u8]) -> Result<Signature> {
            Err(SignError::NoAccount)
        }

        async fn sign_transaction(
            &self,
            _request: MockTxReq,
            _address: &Address,
        ) -> Result<MockSignedTx> {
            Err(SignError::NoAccount)
        }

        fn sign_typed_data(&self, _address: Address, _payload: &TypedData) -> Result<Signature> {
            Err(SignError::NoAccount)
        }
    }

    #[test]
    fn clones_trait_object_with_custom_tx_request_type() {
        let signer: Box<dyn EthSigner<MockSignedTx, MockTxReq>> = Box::new(MockSigner);
        let cloned: Box<dyn EthSigner<MockSignedTx, MockTxReq>> = dyn_clone::clone_box(&*signer);

        assert!(cloned.accounts().is_empty());
    }
}
