//! An abstraction over ethereum signers.

use ethers_core::types::transaction::eip712::TypedData;
use jsonrpsee::core::RpcResult as Result;
use reth_primitives::{Address, Signature, TransactionSigned, U256, H256};
use reth_rpc_types::TypedTransactionRequest;
use secp256k1::{Message, Secp256k1, SecretKey};
use secp256k1::hashes::sha256;
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

    /// Encodes and signs the typed data according EIP-712. Payload must implement Eip712 trait.
    fn sign_typed_data(&self, address: Address, payload: &TypedData) -> Result<Signature>;
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

    async fn sign(&self, address: Address, message: &[u8]) -> Result<Signature> {
        let message = H256::from_slice(message);
        let secp = Secp256k1::new();
        let signer = self.accounts.get(&address).unwrap();
        let message = Message::from_hashed_data::<sha256::Hash>(b"Hello, World!");
        let raw_signature = secp.sign_ecdsa_recoverable(&message, signer);
        let (rec_id, data) = raw_signature.serialize_compact();
        let signature = Signature {
            r: U256::try_from_be_slice(&data[..32]).unwrap(),
            s: U256::try_from_be_slice(&data[32..64]).unwrap(),
            odd_y_parity: rec_id.to_i32() != 0,
        };
        // println!("The signature: {:?}", signature);
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
    use reth_primitives::H256;
    use std::str::FromStr;
    #[tokio::test]
    async fn test_signer() {
        let addresses = vec![];
        let secret =
            SecretKey::from_str("4646464646464646464646464646464646464646464646464646464646464646")
                .unwrap();
        let accounts = HashMap::from([(Address::default(), secret)]);
        let signers = DevSigner { addresses, accounts };
        let message = b"Hello world!";
        let sig = signers.sign(Address::default(), message).await.unwrap();
    }
}
