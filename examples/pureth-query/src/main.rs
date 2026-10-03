use alloy_genesis::{Genesis, GenesisAccount};
use alloy_network::{EthereumWallet, TransactionBuilder};
use alloy_primitives::{Address, Bytes, B256, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer_local::PrivateKeySigner;
use jsonrpsee::{core::client::ClientT, rpc_params};
use reth_chainspec::{ChainSpec, ChainSpecBuilder, MAINNET};
use reth_ethereum::{
    node::{
        builder::NodeBuilder,
        core::{args::RpcServerArgs, node_config::NodeConfig},
        EthereumNode,
    },
    tasks::Runtime,
};
use reth_pureth_query::{
    verify_receipt_log_address, PurethApiServer, PurethRpc, QueryRequest, QueryResponse,
    QueryService, SCHEMA_ID,
};
use reth_pureth_receipt::RethRootProvider;
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    run().await
}

async fn run() -> eyre::Result<()> {
    let mut rpc = RpcServerArgs::default().with_http();
    rpc.ipcdisable = true;
    let signer = PrivateKeySigner::from_bytes(&B256::repeat_byte(0x11))?;
    let contract = Address::repeat_byte(0x22);
    let config = NodeConfig::test()
        .dev()
        .with_chain(chain_spec(signer.address(), contract))
        .with_rpc(rpc)
        .with_unused_ports();
    let node = NodeBuilder::new(config)
        .testing_node(Runtime::test())
        .node(EthereumNode::default())
        .extend_rpc_modules(|ctx| {
            let service = QueryService::from_reth(RethRootProvider::new(ctx.provider().clone()));
            ctx.modules.merge_configured(PurethRpc::new(service).into_rpc())?;
            Ok(())
        })
        .launch_with_debug_capabilities()
        .await?;

    let client = node
        .node
        .rpc_server_handle()
        .http_client()
        .ok_or_else(|| eyre::eyre!("HTTP RPC is disabled"))?;
    let url = node
        .node
        .rpc_server_handle()
        .http_url()
        .ok_or_else(|| eyre::eyre!("HTTP RPC is disabled"))?;
    let signer_address = signer.address();
    let provider =
        ProviderBuilder::new().wallet(EthereumWallet::new(signer)).connect_http(url.parse()?);
    let pending = provider
        .send_transaction(
            TransactionRequest::default()
                .from(signer_address)
                .to(contract)
                .nonce(0)
                .gas_price(2_000_000_000)
                .gas_limit(100_000)
                .with_chain_id(1),
        )
        .await?;
    let receipt = tokio::time::timeout(Duration::from_secs(30), pending.get_receipt()).await??;
    let block_hash =
        receipt.block_hash.ok_or_else(|| eyre::eyre!("mined receipt has no block hash"))?;
    let request = QueryRequest {
        block_hash,
        object: "receipts".to_owned(),
        schema_id: SCHEMA_ID.to_owned(),
        path: "[0].logs[0].address".to_owned(),
        include_proof: true,
    };
    let response: QueryResponse =
        client.request("pureth_query", rpc_params![request.clone()]).await?;
    eyre::ensure!(response.block_hash == block_hash);
    eyre::ensure!(response.schema_id == request.schema_id);
    eyre::ensure!(response.path == request.path);
    eyre::ensure!(response.value_ssz.as_ref() == contract.as_slice());
    eyre::ensure!(response.block_status == "canonical");
    eyre::ensure!(response.root_context == "reth_experimental_unanchored");
    verify_receipt_log_address(
        &request.schema_id,
        &request.path,
        response.value_ssz.as_ref(),
        &response.proof,
        response.root,
    )
    .map_err(|error| eyre::eyre!("{error:?}"))?;

    let missing = QueryRequest { block_hash: B256::repeat_byte(0xff), ..request };
    let error =
        client.request::<QueryResponse, _>("pureth_query", rpc_params![missing]).await.unwrap_err();
    let jsonrpsee::core::client::Error::Call(error) = error else {
        eyre::bail!("expected a missing-block response, got {error:?}")
    };
    eyre::ensure!(error.code() == -32001);
    println!(
        "block={block_hash} address={contract} root={} missing_block_code={}",
        response.root,
        error.code()
    );
    if std::env::var_os("PURETH_EXTERNAL_RPC_CHECK").is_some() {
        println!("http_url={url}");
        tokio::time::sleep(Duration::from_secs(120)).await;
    }
    Ok(())
}

fn chain_spec(signer: Address, contract: Address) -> Arc<ChainSpec> {
    let genesis = Genesis {
        gas_limit: 30_000_000,
        alloc: [
            (signer, GenesisAccount { balance: U256::from(10u128.pow(21)), ..Default::default() }),
            (
                contract,
                GenesisAccount {
                    code: Some(Bytes::from_static(&[0x60, 0x00, 0x60, 0x00, 0xa0, 0x00])),
                    ..Default::default()
                },
            ),
        ]
        .into(),
        ..Default::default()
    };
    Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    )
}

#[cfg(test)]
mod tests {
    #[tokio::test(flavor = "multi_thread")]
    async fn pureth_query_is_installed_on_local_reth_http() {
        super::run().await.unwrap();
    }
}
