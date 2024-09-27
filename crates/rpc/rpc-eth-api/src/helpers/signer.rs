//! An abstraction over ethereum signers.

use alloy_dyn_abi::TypedData;
use alloy_primitives::Address;
use alloy_rpc_types_eth::TransactionRequest;
use dyn_clone::DynClone;
use reth_primitives::{Signature, TransactionSigned};
use reth_rpc_eth_types::SignError;
use std::result;

/// Result returned by [`EthSigner`] methods.
pub type Result<T> = result::Result<T, SignError>;

/// An Ethereum Signer used via RPC.
#[async_trait::async_trait]
pub trait EthSigner: Send + Sync + DynClone {
    /// Returns the available accounts for this signer.
    fn accounts(&self) -> Vec<Address>;

    /// Returns `true` whether this signer can sign for this address
    fn is_signer_for(&self, addr: &Address) -> bool {
        self.accounts().contains(addr)
    }

    /// Returns the signature
    async fn sign(&self, address: Address, message: &[u8]) -> Result<Signature>;

    /// signs a transaction request using the given account in request
    async fn sign_transaction(
        &self,
        request: TransactionRequest,
        address: &Address,
    ) -> Result<TransactionSigned>;

    /// Encodes and signs the typed data according EIP-712. Payload must implement Eip712 trait.
    fn sign_typed_data(&self, address: Address, payload: &TypedData) -> Result<Signature>;
}

dyn_clone::clone_trait_object!(EthSigner);

/// Adds 20 random dev signers for access via the API. Used in dev mode.
#[auto_impl::auto_impl(&)]
pub trait AddDevSigners {
    /// Generates 20 random developer accounts.
    /// Used in DEV mode.
    fn with_dev_accounts(&self);
}
