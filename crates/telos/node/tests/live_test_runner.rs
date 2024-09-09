use std::str::FromStr;
use alloy::consensus::TxLegacy;
use alloy::eips::BlockId;
use alloy::eips::BlockNumberOrTag::Number;
use alloy::network::{Ethereum, EthereumWallet};
use alloy::network::primitives::BlockTransactionsKind;
use alloy::primitives::{Address, TxKind, U256};
use alloy::primitives::TxKind::Create;
use alloy::primitives::utils::{parse_units, Unit};
use alloy::providers::{Identity, Network, Provider, ProviderBuilder, ReqwestProvider};
use alloy::providers::fillers::{FillProvider, JoinFill, WalletFiller};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::transports::http::reqwest::Url;
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy::transports::http::{Client, Http};
use reth::rpc::types::TransactionReceipt;
use reth_primitives::constants::GWEI_TO_WEI;

type SignerProvider = FillProvider<JoinFill<Identity, WalletFiller<EthereumWallet>>, ReqwestProvider, Http<Client>, Ethereum>;

#[tokio::test]
pub async fn run_local() {
    env_logger::builder().is_test(true).try_init().unwrap();
    let url = "http://localhost:8545";
    let private_key = "26e86e45f6fc45ec6e2ecd128cec80fa1d1505e5507dcd2ae58c3130a7a97b48";
    run_tests(url, private_key).await;
}

pub async fn run_tests(url: &str, private_key: &str) {
    let signer = PrivateKeySigner::from_str(private_key).unwrap();
    let address = signer.address();
    let wallet = EthereumWallet::from(signer);

    let provider = ProviderBuilder::new().wallet(wallet.clone()).on_http(Url::from_str(url).unwrap());

    let block = provider.get_block(BlockId::Number(Number(1)), BlockTransactionsKind::Full).await;
    println!("{:?}", block);

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

    let (address, wallet, provider) = create_provider(url, private_key);
    println!("Deploying contract using address {address}");

    let nonce = provider.get_transaction_count(address).await.unwrap();
    let chain_id = provider.get_chain_id().await.unwrap();
    let gas_price: u128 = 600 * (GWEI_TO_WEI as u128);

    let legacy_tx = TxLegacy {
        chain_id: Some(chain_id),
        nonce,
        gas_price: gas_price.into(),
        gas_limit: 1_000_000,
        to: Create,
        value: U256::ZERO,
        input: BlockNumChecker::BYTECODE.clone().into(),
    };

    let mut legacy_tx_request: <Ethereum as Network>::TransactionRequest = legacy_tx.into();
    legacy_tx_request.to = Some(Create);
    let deploy_result = provider.send_transaction(legacy_tx_request).await.unwrap();

    let deploy_tx_hash = deploy_result.tx_hash();
    println!("Deployed contract with tx hash: {deploy_tx_hash}");
    let receipt = deploy_result.get_receipt().await.unwrap();
    println!("Receipt: {:?}", receipt);
}

fn create_provider(url: &str, private_key: &str) -> (Address, EthereumWallet, SignerProvider) {
    let signer = PrivateKeySigner::from_str(private_key).unwrap();
    let address = signer.address();
    let wallet = EthereumWallet::from(signer);

    let provider: SignerProvider = ProviderBuilder::new().wallet(wallet.clone()).on_http(Url::from_str(url).unwrap());
    (address, wallet, provider)
}