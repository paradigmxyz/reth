//! An abstraction over ethereum signers.

use crate::eth::error::SignError;
use alloy_dyn_abi::TypedData;
use reth_primitives::{
    eip191_hash_message, sign_message, Address, Signature, TransactionSigned, B256,
};
use reth_rpc_types::TypedTransactionRequest;

use dyn_clone::DynClone;
use reth_rpc_types_compat::transaction::to_primitive_transaction;
use secp256k1::SecretKey;
use std::collections::HashMap;

type Result<T> = std::result::Result<T, SignError>;

/// An Ethereum Signer used via RPC.
#[async_trait::async_trait]
pub(crate) trait EthSigner: Send + Sync + DynClone {
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

dyn_clone::clone_trait_object!(EthSigner);

/// Holds developer keys
#[derive(Clone)]
pub(crate) struct DevSigner {
    addresses: Vec<Address>,
    accounts: HashMap<Address, SecretKey>,
}

#[allow(dead_code)]
impl DevSigner {
    /// Generates a random dev signer which satisfies [EthSigner] trait
    pub(crate) fn random() -> Box<dyn EthSigner> {
        let mut signers = Self::random_signers(1);
        signers.pop().expect("expect to generate at leas one signer")
    }

    /// Generates provided number of random dev signers
    /// which satisfy [EthSigner] trait
    pub(crate) fn random_signers(num: u32) -> Vec<Box<dyn EthSigner + 'static>> {
        let mut signers = Vec::new();
        for _ in 0..num {
            let (sk, pk) = secp256k1::generate_keypair(&mut rand::thread_rng());

            let address = reth_primitives::public_key_to_address(pk);
            let addresses = vec![address];
            let accounts = HashMap::from([(address, sk)]);
            signers.push(Box::new(DevSigner { addresses, accounts }) as Box<dyn EthSigner>);
        }
        signers
    }

    fn get_key(&self, account: Address) -> Result<&SecretKey> {
        self.accounts.get(&account).ok_or(SignError::NoAccount)
    }

    fn sign_hash(&self, hash: B256, account: Address) -> Result<Signature> {
        let secret = self.get_key(account)?;
        let signature = sign_message(B256::from_slice(secret.as_ref()), hash);
        signature.map_err(|_| SignError::CouldNotSign)
    }
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
        // Hash message according to EIP 191:
        // https://ethereum.org/es/developers/docs/apis/json-rpc/#eth_sign
        let hash = eip191_hash_message(message);
        self.sign_hash(hash, address)
    }

    fn sign_transaction(
        &self,
        request: TypedTransactionRequest,
        address: &Address,
    ) -> Result<TransactionSigned> {
        // convert to primitive transaction
        let transaction =
            to_primitive_transaction(request).ok_or(SignError::InvalidTransactionRequest)?;
        let tx_signature_hash = transaction.signature_hash();
        let signature = self.sign_hash(tx_signature_hash, *address)?;

        Ok(TransactionSigned::from_transaction_and_signature(transaction, signature))
    }

    fn sign_typed_data(&self, address: Address, payload: &TypedData) -> Result<Signature> {
        let encoded = payload.eip712_signing_hash().map_err(|_| SignError::InvalidTypedData)?;
        // let b256 = encoded;
        self.sign_hash(encoded, address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::U256;
    use std::str::FromStr;
    fn build_signer() -> DevSigner {
        let addresses = vec![];
        let secret =
            SecretKey::from_str("4646464646464646464646464646464646464646464646464646464646464646")
                .unwrap();
        let accounts = HashMap::from([(Address::default(), secret)]);
        DevSigner { addresses, accounts }
    }

    #[tokio::test]
    async fn test_sign_type_data() {
        let eip_712_example = serde_json::json!(
            r#"{
            "types": {
            "EIP712Domain": [
                {
                    "name": "name",
                    "type": "string"
                },
                {
                    "name": "version",
                    "type": "string"
                },
                {
                    "name": "chainId",
                    "type": "uint256"
                },
                {
                    "name": "verifyingContract",
                    "type": "address"
                }
            ],
            "Person": [
                {
                    "name": "name",
                    "type": "string"
                },
                {
                    "name": "wallet",
                    "type": "address"
                }
            ],
            "Mail": [
                {
                    "name": "from",
                    "type": "Person"
                },
                {
                    "name": "to",
                    "type": "Person"
                },
                {
                    "name": "contents",
                    "type": "string"
                }
            ]
        },
        "primaryType": "Mail",
        "domain": {
            "name": "Ether Mail",
            "version": "1",
            "chainId": 1,
            "verifyingContract": "0xCcCCccccCCCCcCCCCCCcCcCccCcCCCcCcccccccC"
        },
        "message": {
            "from": {
                "name": "Cow",
                "wallet": "0xCD2a3d9F938E13CD947Ec05AbC7FE734Df8DD826"
            },
            "to": {
                "name": "Bob",
                "wallet": "0xbBbBBBBbbBBBbbbBbbBbbbbBBbBbbbbBbBbbBBbB"
            },
            "contents": "Hello, Bob!"
        }
        }"#
        );
        let data: TypedData = serde_json::from_value(eip_712_example).unwrap();
        let signer = build_signer();
        let sig = signer.sign_typed_data(Address::default(), &data).unwrap();
        let expected = Signature {
            r: U256::from_str_radix(
                "5318aee9942b84885761bb20e768372b76e7ee454fc4d39b59ce07338d15a06c",
                16,
            )
            .unwrap(),
            s: U256::from_str_radix(
                "5e585a2f4882ec3228a9303244798b47a9102e4be72f48159d890c73e4511d79",
                16,
            )
            .unwrap(),
            odd_y_parity: false,
        };
        assert_eq!(sig, expected)
    }

    #[tokio::test]
    async fn test_signer() {
        let message = b"Test message";
        let signer = build_signer();
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
