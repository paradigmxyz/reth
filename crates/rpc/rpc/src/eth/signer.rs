//! An abstraction over ethereum signers.

use crate::eth::error::SignError;
use ethers_core::{
    types::transaction::eip712::{Eip712, TypedData},
    utils::hash_message,
};
use reth_primitives::{
    sign_message, Address, Signature, Transaction as PrimitiveTransaction, TransactionSigned,
    TxEip1559, TxEip2930, TxLegacy, H256,
};
use reth_rpc_types::TypedTransactionRequest;

use secp256k1::SecretKey;
use std::collections::HashMap;

type Result<T> = std::result::Result<T, SignError>;

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

impl DevSigner {
    fn get_key(&self, account: Address) -> Result<&SecretKey> {
        self.accounts.get(&account).ok_or(SignError::NoAccount)
    }
    fn sign_hash(&self, hash: H256, account: Address) -> Result<Signature> {
        let secret = self.get_key(account)?;
        let signature = sign_message(H256::from_slice(secret.as_ref()), hash);
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
        let hash = hash_message(message).into();
        self.sign_hash(hash, address)
    }

    fn sign_transaction(
        &self,
        _request: TypedTransactionRequest,
        _address: &Address,
    ) -> Result<TransactionSigned> {
        // convert to primitive transaction
        let mut _transaction = match _request {
            TypedTransactionRequest::Legacy(tx) => PrimitiveTransaction::Legacy(TxLegacy {
                chain_id: tx.chain_id,
                nonce: u64::from_be_bytes(tx.nonce.to_be_bytes()),
                gas_price: u128::from_be_bytes(tx.gas_price.to_be_bytes()),
                gas_limit: u64::from_be_bytes(tx.gas_limit.to_be_bytes()),
                to: tx.kind.into(),
                value: u128::from_be_bytes(tx.value.to_be_bytes()),
                input: tx.input,
            }),
            TypedTransactionRequest::EIP2930(tx) => PrimitiveTransaction::Eip2930(TxEip2930 {
                chain_id: tx.chain_id,
                nonce: u64::from_be_bytes(tx.nonce.to_be_bytes()),
                gas_price: u128::from_be_bytes(tx.gas_price.to_be_bytes()),
                gas_limit: u64::from_be_bytes(tx.gas_limit.to_be_bytes()),
                to: tx.kind.into(),
                value: u128::from_be_bytes(tx.value.to_be_bytes()),
                input: tx.input,
                access_list: tx.access_list,
            }),
            TypedTransactionRequest::EIP1559(tx) => PrimitiveTransaction::Eip1559(TxEip1559 {
                chain_id: tx.chain_id,
                nonce: u64::from_be_bytes(tx.nonce.to_be_bytes()),
                max_fee_per_gas: u128::from_be_bytes(tx.max_fee_per_gas.to_be_bytes()),
                gas_limit: u64::from_be_bytes(tx.gas_limit.to_be_bytes()),
                to: tx.kind.into(),
                value: u128::from_be_bytes(tx.value.to_be_bytes()),
                input: tx.input,
                access_list: tx.access_list,
                max_priority_fee_per_gas: u128::from_be_bytes(
                    tx.max_priority_fee_per_gas.to_be_bytes(),
                ),
            }),
        };

        let tx_signature_hash = _transaction.signature_hash();
        let signature = self.sign_hash(tx_signature_hash, *_address)?;

        Ok(TransactionSigned::from_transaction_and_signature(_transaction, signature))
    }

    fn sign_typed_data(&self, address: Address, payload: &TypedData) -> Result<Signature> {
        let encoded: H256 = payload.encode_eip712().map_err(|_| SignError::TypedData)?.into();
        self.sign_hash(encoded, address)
    }
}
#[cfg(test)]
mod test {
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
