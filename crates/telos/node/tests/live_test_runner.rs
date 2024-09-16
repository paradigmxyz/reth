use std::str::FromStr;
use alloy_network::{Ethereum, Network};
use alloy_provider::network::EthereumWallet;
use alloy_provider::{Provider, ProviderBuilder, ReqwestProvider};
use alloy_provider::fillers::{FillProvider, JoinFill, WalletFiller};
use alloy_rpc_types::TransactionRequest;
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::private::primitives::TxKind::Create;
use alloy_sol_types::{sol, SolValue};
use alloy_transport_http::Http;
use reth::primitives::{AccessList, Address, BlockId, U256};

use reth::rpc::types::{BlockTransactionsKind, TransactionInput, TransactionReceipt};
use reth_primitives::constants::GWEI_TO_WEI;
use reth_primitives::TxLegacy;
use reqwest::{Client, Url};
use tracing::info;
use reth::primitives::BlockNumberOrTag::Number;
use reth::rpc::builder::Identity;

#[tokio::test]
pub async fn run_local() {
    env_logger::builder().is_test(true).try_init().unwrap();
    let url = "http://localhost:8545";
    let private_key = "26e86e45f6fc45ec6e2ecd128cec80fa1d1505e5507dcd2ae58c3130a7a97b48";
    run_tests(url, private_key).await;
}

pub async fn run_tests(url: &str, private_key: &str) {
    let signer = PrivateKeySigner::from_str(private_key).unwrap();
    let wallet = EthereumWallet::from(signer.clone());
    let provider = ProviderBuilder::new().wallet(wallet.clone()).on_http(Url::from_str(url).unwrap());
    let signer_address = signer.address();
    let balance = provider.get_balance(signer_address).await.unwrap();

    info!("Running live tests using address: {:?} with balance: {:?}", signer_address, balance);

    let block = provider.get_block(BlockId::latest(), BlockTransactionsKind::Full).await;
    info!("Latest block:\n {:?}", block);

    test_blocknum_onchain(url, private_key).await;
}

pub async fn test_blocknum_onchain(url: &str, private_key: &str) {
    sol! {
        #[sol(rpc, bytecode="6080604052348015600e575f80fd5b5060ef8061001b5f395ff3fe6080604052348015600e575f80fd5b50600436106030575f3560e01c80637f6c6f101460345780638fb82b0214604e575b5f80fd5b603a6056565b6040516045919060a2565b60405180910390f35b6054605d565b005b5f43905090565b437fc04eeb4cfe0799838abac8fa75bca975bff679179886c80c84a7b93229a1a61860405160405180910390a2565b5f819050919050565b609c81608c565b82525050565b5f60208201905060b35f8301846095565b9291505056fea264697066735822122003482ecf0ea4d820deb6b5ebd2755b67c3c8d4fb9ed50a8b4e0bce59613552df64736f6c634300081a0033")]
        contract BlockNumChecker {

            event BlockNumber(uint256 indexed number);

            function getBlockNum() public view returns (uint) {
                return block.number;
            }

            function logBlockNum() public {
                emit BlockNumber(block.number);
            }
        }
    }

    let signer = PrivateKeySigner::from_str(private_key).unwrap();
    let address = signer.address();
    let wallet = EthereumWallet::from(signer);

    let provider = ProviderBuilder::new().wallet(wallet.clone()).on_http(Url::from_str(url).unwrap());


    info!("Deploying contract using address {address}");

    let nonce = provider.get_transaction_count(address).await.unwrap();
    let chain_id = provider.get_chain_id().await.unwrap();
    let gas_price: u128 = 600 * (GWEI_TO_WEI as u128);

    let legacy_tx = TxLegacy {
        chain_id: Some(chain_id),
        nonce,
        gas_price: gas_price.into(),
        gas_limit: 20_000_000,
        to: reth::primitives::TxKind::Create,
        value: U256::ZERO,
        input: BlockNumChecker::BYTECODE.to_vec().into(),
    };

    let legacy_tx_request = TransactionRequest {
        from: Some(address),
        to: Some(legacy_tx.to),
        gas: Some(legacy_tx.gas_limit as u128),
        gas_price: Some(legacy_tx.gas_price),
        value: Some(legacy_tx.value),
        input: TransactionInput::from(legacy_tx.input),
        nonce: Some(legacy_tx.nonce),
        access_list: Some(AccessList::default()),
        chain_id: legacy_tx.chain_id,
        ..Default::default()
    };

    let deploy_result = provider.send_transaction(legacy_tx_request).await.unwrap();

    let deploy_tx_hash = deploy_result.tx_hash();
    info!("Deployed contract with tx hash: {deploy_tx_hash}");
    let receipt = deploy_result.get_receipt().await.unwrap();
    info!("Receipt: {:?}", receipt);
}