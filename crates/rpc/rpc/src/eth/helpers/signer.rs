//! An abstraction over ethereum signers.

use std::collections::HashMap;

use crate::EthApi;
use alloy_dyn_abi::TypedData;
use alloy_eips::eip2718::Decodable2718;
use alloy_network::{eip2718::Encodable2718, EthereumWallet, TransactionBuilder};
use alloy_primitives::{eip191_hash_message, Address, B256};
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use reth_primitives::{Signature, TransactionSigned};
use reth_rpc_eth_api::helpers::{signer::Result, AddDevSigners, EthSigner};
use reth_rpc_eth_types::SignError;

impl<Provider, Pool, Network, EvmConfig> AddDevSigners
    for EthApi<Provider, Pool, Network, EvmConfig>
{
    fn with_dev_accounts(&self) {
        *self.inner.signers().write() = DevSigner::random_signers(20)
    }
}

/// Holds developer keys
#[derive(Debug, Clone)]
pub struct DevSigner {
    addresses: Vec<Address>,
    accounts: HashMap<Address, PrivateKeySigner>,
}

#[allow(dead_code)]
impl DevSigner {
    /// Generates a random dev signer which satisfies [`EthSigner`] trait
    pub fn random() -> Box<dyn EthSigner> {
        let mut signers = Self::random_signers(1);
        signers.pop().expect("expect to generate at least one signer")
    }

    /// Generates provided number of random dev signers
    /// which satisfy [`EthSigner`] trait
    pub fn random_signers(num: u32) -> Vec<Box<dyn EthSigner + 'static>> {
        let mut signers = Vec::new();
        for _ in 0..num {
            let sk = PrivateKeySigner::random_with(&mut rand::thread_rng());

            let address = sk.address();
            let addresses = vec![address];

            let accounts = HashMap::from([(address, sk)]);
            signers.push(Box::new(Self { addresses, accounts }) as Box<dyn EthSigner>);
        }
        signers
    }

    fn get_key(&self, account: Address) -> Result<&PrivateKeySigner> {
        self.accounts.get(&account).ok_or(SignError::NoAccount)
    }

    fn sign_hash(&self, hash: B256, account: Address) -> Result<Signature> {
        let signature = self.get_key(account)?.sign_hash_sync(&hash);
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

    async fn sign_transaction(
        &self,
        request: TransactionRequest,
        address: &Address,
    ) -> Result<TransactionSigned> {
        // create local signer wallet from signing key
        let signer = self.accounts.get(address).ok_or(SignError::NoAccount)?.clone();
        let wallet = EthereumWallet::from(signer);

        // build and sign transaction with signer
        let txn_envelope =
            request.build(&wallet).await.map_err(|_| SignError::InvalidTransactionRequest)?;

        // decode transaction into signed transaction type
        let encoded = txn_envelope.encoded_2718();
        let txn_signed = TransactionSigned::decode_2718(&mut encoded.as_ref())
            .map_err(|_| SignError::InvalidTransactionRequest)?;

        Ok(txn_signed)
    }

    fn sign_typed_data(&self, address: Address, payload: &TypedData) -> Result<Signature> {
        let encoded = payload.eip712_signing_hash().map_err(|_| SignError::InvalidTypedData)?;
        self.sign_hash(encoded, address)
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Bytes, Parity, U256};
    use alloy_rpc_types_eth::TransactionInput;
    use revm_primitives::TxKind;

    use super::*;

    fn build_signer() -> DevSigner {
        let signer: PrivateKeySigner =
            "4646464646464646464646464646464646464646464646464646464646464646".parse().unwrap();
        let address = signer.address();
        let accounts = HashMap::from([(address, signer)]);
        let addresses = vec![address];
        DevSigner { addresses, accounts }
    }

    #[tokio::test]
    async fn test_sign_type_data() {
        let eip_712_example = r#"{
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
        }"#;
        let data: TypedData = serde_json::from_str(eip_712_example).unwrap();
        let signer = build_signer();
        let from = *signer.addresses.first().unwrap();
        let sig = signer.sign_typed_data(from, &data).unwrap();
        let expected = Signature::new(
            U256::from_str_radix(
                "5318aee9942b84885761bb20e768372b76e7ee454fc4d39b59ce07338d15a06c",
                16,
            )
            .unwrap(),
            U256::from_str_radix(
                "5e585a2f4882ec3228a9303244798b47a9102e4be72f48159d890c73e4511d79",
                16,
            )
            .unwrap(),
            Parity::Parity(false),
        );
        assert_eq!(sig, expected)
    }

    #[tokio::test]
    async fn test_signer() {
        let message = b"Test message";
        let signer = build_signer();
        let from = *signer.addresses.first().unwrap();
        let sig = signer.sign(from, message).await.unwrap();
        let expected = Signature::new(
            U256::from_str_radix(
                "54313da7432e4058b8d22491b2e7dbb19c7186c35c24155bec0820a8a2bfe0c1",
                16,
            )
            .unwrap(),
            U256::from_str_radix(
                "687250f11a3d4435004c04a4cb60e846bc27997271d67f21c6c8170f17a25e10",
                16,
            )
            .unwrap(),
            Parity::Parity(true),
        );
        assert_eq!(sig, expected)
    }

    #[tokio::test]
    async fn test_sign_transaction() {
        let message = b"Test message";
        let signer = build_signer();
        let from = *signer.addresses.first().unwrap();
        let request = TransactionRequest {
            chain_id: Some(1u64),
            from: Some(from),
            to: Some(TxKind::Create),
            gas: Some(1000),
            gas_price: Some(1000u128),
            value: Some(U256::from(1000)),
            input: TransactionInput {
                data: Some(Bytes::from(message.to_vec())),
                input: Some(Bytes::from(message.to_vec())),
            },
            nonce: Some(0u64),
            ..Default::default()
        };
        let txn_signed = signer.sign_transaction(request, &from).await;
        assert!(txn_signed.is_ok());

        assert_eq!(Bytes::from(message.to_vec()), txn_signed.unwrap().input().0);
    }
}
