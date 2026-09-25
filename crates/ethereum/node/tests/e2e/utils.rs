use alloy_eips::{eip2930::AccessListItem, eip7702::Authorization, BlockId, BlockNumberOrTag};
use alloy_primitives::{bytes, U256};
use alloy_provider::{
    network::{
        Ethereum, EthereumWallet, NetworkWallet, TransactionBuilder, TransactionBuilder7702,
    },
    Provider, SendableTx,
};
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer::SignerSync;
use eyre::{ensure, eyre};
use rand::{seq::IndexedRandom, Rng};
use reqwest::{header, RequestBuilder, Response, StatusCode};
use reth_e2e_test_utils::{wallet::Wallet, NodeHelperType};
use reth_ethereum_primitives::TxType;
use reth_node_ethereum::EthereumNode;
use reth_rpc_builder::auth::AuthServerHandle;
use reth_rpc_layer::secret_to_bearer_header;
use ssz::{Decode, Encode};

/// Advances node by producing blocks with random transactions.
pub(crate) async fn advance_with_random_transactions(
    node: &mut NodeHelperType<EthereumNode>,
    num_blocks: usize,
    rng: &mut impl Rng,
    finalize: bool,
) -> eyre::Result<()> {
    let provider = node.rpc_provider();
    let signers = Wallet::new(1).with_chain_id(provider.get_chain_id().await?).wallet_gen();

    // simple contract which writes to storage on any call
    let dummy_bytecode = bytes!(
        "6080604052348015600f57600080fd5b50602880601d6000396000f3fe4360a09081523360c0526040608081905260e08152902080805500fea164736f6c6343000810000a"
    );
    let mut call_destinations = signers.iter().map(|s| s.address()).collect::<Vec<_>>();

    for _ in 0..num_blocks {
        let tx_count = rng.random_range(1..20);

        let mut pending = vec![];
        for _ in 0..tx_count {
            let signer = signers.choose(rng).unwrap();
            let tx_type = TxType::try_from(rng.random_range(0..=4) as u64).unwrap();

            let nonce = provider
                .get_transaction_count(signer.address())
                .block_id(BlockId::Number(BlockNumberOrTag::Pending))
                .await?;

            let mut tx =
                TransactionRequest::default().with_from(signer.address()).with_nonce(nonce);

            let should_create =
                rng.random::<bool>() && tx_type != TxType::Eip4844 && tx_type != TxType::Eip7702;
            if should_create {
                tx = tx.into_create().with_input(dummy_bytecode.clone());
            } else {
                tx = tx.with_to(*call_destinations.choose(rng).unwrap()).with_input(
                    (0..rng.random_range(0..10000)).map(|_| rng.random()).collect::<Vec<u8>>(),
                );
            }

            if matches!(tx_type, TxType::Legacy | TxType::Eip2930) {
                tx = tx.with_gas_price(provider.get_gas_price().await?);
            }

            if rng.random::<bool>() || tx_type == TxType::Eip2930 {
                tx = tx.with_access_list(
                    vec![AccessListItem {
                        address: *call_destinations.choose(rng).unwrap(),
                        storage_keys: (0..rng.random_range(0..100)).map(|_| rng.random()).collect(),
                    }]
                    .into(),
                );
            }

            if tx_type == TxType::Eip7702 {
                let signer = signers.choose(rng).unwrap();
                let auth = Authorization {
                    chain_id: U256::from(provider.get_chain_id().await?),
                    address: *call_destinations.choose(rng).unwrap(),
                    nonce: provider
                        .get_transaction_count(signer.address())
                        .block_id(BlockId::Number(BlockNumberOrTag::Pending))
                        .await?,
                };
                let sig = signer.sign_hash_sync(&auth.signature_hash())?;
                tx = tx.with_authorization_list(vec![auth.into_signed(sig)])
            }

            let gas = provider
                .estimate_gas(tx.clone())
                .block(BlockId::Number(BlockNumberOrTag::Pending))
                .await
                .unwrap_or(1_000_000);

            tx.set_gas_limit(gas);

            let SendableTx::Builder(tx) = provider.fill(tx).await? else { unreachable!() };
            let tx =
                NetworkWallet::<Ethereum>::sign_request(&EthereumWallet::new(signer.clone()), tx)
                    .await?;

            if let Ok(res) = provider.send_tx_envelope(tx).await {
                pending.push(res);
            }
        }

        let payload = node.build_and_submit_payload().await?;
        if finalize {
            node.update_forkchoice(payload.block().hash(), payload.block().hash()).await?;
        } else {
            let last_safe =
                provider.get_block_by_number(BlockNumberOrTag::Safe).await?.unwrap().header.hash;
            node.update_forkchoice(last_safe, payload.block().hash()).await?;
        }

        for pending in pending {
            let receipt = pending.get_receipt().await?;
            if let Some(address) = receipt.contract_address {
                call_destinations.push(address);
            }
        }
    }

    Ok(())
}

/// Header selecting the fork of fork-scoped SSZ engine API endpoints.
pub(crate) const ENGINE_EXECUTION_VERSION_HEADER: &str = "Eth-Execution-Version";

/// Extension trait for requests against the SSZ engine API.
pub(crate) trait EngineSszRequestExt {
    /// Authenticates the request with the JWT secret of the auth server.
    fn jwt(self, auth: &AuthServerHandle) -> Self;

    /// Selects the fork of a fork-scoped endpoint.
    fn fork(self, fork: &str) -> Self;

    /// Sets the SSZ encoded body.
    fn ssz(self, body: &impl Encode) -> Self;
}

impl EngineSszRequestExt for RequestBuilder {
    fn jwt(self, auth: &AuthServerHandle) -> Self {
        self.header(header::AUTHORIZATION, secret_to_bearer_header(auth.jwt_secret()))
    }

    fn fork(self, fork: &str) -> Self {
        self.header(ENGINE_EXECUTION_VERSION_HEADER, fork)
    }

    fn ssz(self, body: &impl Encode) -> Self {
        self.header(header::CONTENT_TYPE, "application/octet-stream").body(body.as_ssz_bytes())
    }
}

/// Extension trait for responses of the SSZ engine API.
pub(crate) trait EngineSszResponseExt {
    /// Decodes the SSZ body of a `200 OK` response.
    async fn ssz<T: Decode>(self) -> eyre::Result<T>;

    /// Returns the `type` of a problem details error response.
    async fn problem_type(self) -> eyre::Result<String>;
}

impl EngineSszResponseExt for Response {
    async fn ssz<T: Decode>(self) -> eyre::Result<T> {
        let status = self.status();
        let bytes = self.bytes().await?;
        ensure!(status == StatusCode::OK, "{status}: {}", String::from_utf8_lossy(&bytes));
        T::from_ssz_bytes(&bytes).map_err(|err| eyre!("failed to decode SSZ response: {err:?}"))
    }

    async fn problem_type(self) -> eyre::Result<String> {
        let content_type = self.headers().get(header::CONTENT_TYPE).cloned();
        ensure!(
            content_type.as_ref().is_some_and(|value| value == "application/problem+json"),
            "expected a problem details response, got {content_type:?}"
        );
        let problem = self.json::<serde_json::Value>().await?;
        problem["type"].as_str().map(str::to_owned).ok_or_else(|| eyre!("missing type: {problem}"))
    }
}
