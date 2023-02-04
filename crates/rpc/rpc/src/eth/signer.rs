//! An abstraction over ethereum signers.

use jsonrpsee::core::RpcResult as Result;
use reth_primitives::{Address, Signature, TransactionSigned};
use reth_rpc_types::TypedTransactionRequest;
use secp256k1::SecretKey;
use std::collections::HashMap;

/// An Ethereum Signer used via RPC.
#[async_trait::async_trait]
pub(crate) trait EthSigner: Send + Sync {
    /// Returns the available accounts for this signer.
    fn accounts(&self) -> Vec<Address>;

    /// Returns `true` whether this signer can sign for this address
    fn is_signer_for(&self, addr: &Address) -> bool {
        self.accounts().contains(addr)
    }

    /// Returns the signature
    async fn sign(&self, address: Address, message: &[u8]) -> Result<Signature>;

    /// signs a transaction request using the given account in request
    fn sign_transaction(
        &self,
        request: TypedTransactionRequest,
        address: &Address,
    ) -> Result<TransactionSigned>;
}

/// Holds developer keys
pub(crate) struct DevSigner {
    addresses: Vec<Address>,
    accounts: HashMap<Address, SecretKey>,
}

#[async_trait::async_trait]
impl EthSigner for DevSigner {
    fn accounts(&self) -> Vec<Address> {
        self.addresses.clone()
    }

    fn is_signer_for(&self, addr: &Address) -> bool {
        self.accounts.contains_key(addr)
    }

    async fn sign(&self, _address: Address, _message: &[u8]) -> Result<Signature> {
        todo!()
    }

    fn sign_transaction(
        &self,
        _request: TypedTransactionRequest,
        _address: &Address,
    ) -> Result<TransactionSigned> {
        todo!()
    }
}
