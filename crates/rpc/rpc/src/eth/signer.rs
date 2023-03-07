//! An abstraction over ethereum signers.

use ethers_core::types::transaction::eip712::TypedData;
use jsonrpsee::core::{Error as RpcError, RpcResult as Result};
use reth_primitives::{Address, Signature, TransactionSigned, U256};
use reth_rpc_types::TypedTransactionRequest;
use secp256k1::{hashes::{sha256, Hash}, Message, Secp256k1, SecretKey};
use std::collections::HashMap;
use ethers_core::utils::hash_message;

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

    /// Encodes and signs the typed data according EIP-712. Payload must implement Eip712 trait.
    fn sign_typed_data(&self, address: Address, payload: &TypedData) -> Result<Signature>;
}

/// Holds developer keys
pub(crate) struct DevSigner {
    pub(crate) addresses: Vec<Address>,
    pub(crate) accounts: HashMap<Address, SecretKey>,
}

#[async_trait::async_trait]
impl EthSigner for DevSigner {
    fn accounts(&self) -> Vec<Address> {
        self.addresses.clone()
    }

    fn is_signer_for(&self, addr: &Address) -> bool {
        self.accounts.contains_key(addr)
    }

    async fn sign(&self, address: Address, message: &[u8]) -> Result<Signature> {
        let secp = Secp256k1::new();
        let secret =
            self.accounts.get(&address).ok_or(RpcError::Custom("No account".to_string()))?;
        // TODO:
        // Not sure on how to properly handle
        // this unwrap.
        // Hash message according to EIP 191.
        let message = Message::from_slice(hash_message(&message).as_bytes()).unwrap();
        let (rec_id, data) = secp.sign_ecdsa_recoverable(&message, secret).serialize_compact();
        let signature = Signature {
            // TODO:
            // Not sure on how to properly handle
            // this unwraps
            r: U256::try_from_be_slice(&data[..32]).unwrap(),
            s: U256::try_from_be_slice(&data[32..64]).unwrap(),
            odd_y_parity: rec_id.to_i32() != 0,
        };
        Ok(signature)
    }

    fn sign_transaction(
        &self,
        _request: TypedTransactionRequest,
        _address: &Address,
    ) -> Result<TransactionSigned> {
        todo!()
    }
    fn sign_typed_data(&self, _address: Address, _payload: &TypedData) -> Result<Signature> {
        todo!()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::str::FromStr;
    #[tokio::test]
    async fn test_signer() {
        let addresses = vec![];
        let secret =
            SecretKey::from_str("4646464646464646464646464646464646464646464646464646464646464646")
                .unwrap();
        let accounts = HashMap::from([(Address::default(), secret)]);
        let signer = DevSigner { addresses, accounts };
        let message = b"Test message";
        let sig = signer.sign(Address::default(), message).await.unwrap();
        let expected = Signature {
            r: U256::from_str_radix(
                "bef421631867801a4b2d9c4241fde50aa82fecc020e0bb80c18fd41b6113b063",
                16,
            )
            .unwrap(),
            s: U256::from_str_radix(
                "5a83250a694e5255ff1a3a7610204f0e7329c43bd9d1d80c35c1238fc5a570a2",
                16,
            )
            .unwrap(),
            odd_y_parity: false,
        };
        assert_eq!(sig, expected)
    }
}
