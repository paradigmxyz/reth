use jsonrpsee::{core::client::ClientT, rpc_params};
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
use reth_pureth_receipt::SINGLETON_BLOCK_HASH;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    run().await
}

async fn run() -> eyre::Result<()> {
    let mut rpc = RpcServerArgs::default().with_http();
    rpc.ipcdisable = true;
    let config = NodeConfig::test().dev().with_rpc(rpc).with_unused_ports();
    let node = NodeBuilder::new(config)
        .testing_node(Runtime::test())
        .node(EthereumNode::default())
        .extend_rpc_modules(|ctx| {
            let service = QueryService::new().map_err(|error| eyre::eyre!("{error:?}"))?;
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
    let request = QueryRequest {
        block_hash: SINGLETON_BLOCK_HASH,
        object: "receipts".to_owned(),
        schema_id: SCHEMA_ID.to_owned(),
        path: "[0].logs[0].address".to_owned(),
        include_proof: true,
    };
    let response: QueryResponse = client.request("pureth_query", rpc_params![request]).await?;
    eyre::ensure!(response.block_hash == SINGLETON_BLOCK_HASH);
    eyre::ensure!(response.value_ssz.as_ref() == &[0x11; 20]);
    eyre::ensure!(response.gindex == "576");
    eyre::ensure!(
        response.root.to_string() ==
            "0x5036e5a260a45255df46094662d27826bb1f417e3dccb4b3a4fc313876cd4e33"
    );
    verify_receipt_log_address(
        &response.schema_id,
        &response.path,
        response.value_ssz.as_ref(),
        &response.proof,
        response.root,
    )
    .map_err(|error| eyre::eyre!("{error:?}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test(flavor = "multi_thread")]
    async fn pureth_query_is_installed_on_local_reth_http() {
        super::run().await.unwrap();
    }
}
