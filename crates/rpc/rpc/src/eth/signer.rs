//! An abstraction over ethereum signers.

use ethers_core::{types::transaction::eip712::TypedData, utils::hash_message};
use jsonrpsee::core::{Error as RpcError, RpcResult as Result};
use reth_primitives::{Address, Signature, TransactionSigned, U256};
use reth_rpc_types::TypedTransactionRequest;
use secp256k1::{Message, Secp256k1, SecretKey};
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
        let secp = Secp256k1::new();
        let secret =
            self.accounts.get(&address).ok_or(RpcError::Custom("No account".to_string()))?;
        // Hash message according to EIP 191:
        // https://ethereum.org/es/developers/docs/apis/json-rpc/#eth_sign
        // TODO: Handle unwrap properly
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
                "54313da7432e4058b8d22491b2e7dbb19c7186c35c24155bec0820a8a2bfe0c1",
                16,
            )
            .unwrap(),
            s: U256::from_str_radix(
                "687250f11a3d4435004c04a4cb60e846bc27997271d67f21c6c8170f17a25e10",
                16,
            )
            .unwrap(),
            odd_y_parity: true,
        };
        assert_eq!(sig, expected)
    }
}
