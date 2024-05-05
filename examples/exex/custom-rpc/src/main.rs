`

use clap::{Id, Parser};
use jsonrpsee::{core::{RpcResult},types::{error::ErrorObject}, proc_macros::rpc};
use reth::{cli::Cli, rpc::builder::error::RpcError};
use reth_node_ethereum::EthereumNode;
use alloy_sol_types::{sol, SolEventInterface};
use reth_provider::DatabaseProviderFactory;
use async_trait::async_trait;
use reth_rpc_api::{EthFilterApiClient, EthFilterApiServer};
use alloy_rpc_types::{Filter,BlockNumberOrTag};
sol!(L1StandardBridge, "l1_standard_bridge_abi.json");
use crate::L1StandardBridge::{ETHBridgeFinalized, ETHBridgeInitiated, L1StandardBridgeEvents};

fn main() {
    Cli::<RethCliOpDepositCountExt>::parse()
        .run(|builder, args| async move {
            let handle = builder
                .node(EthereumNode::default())
                .extend_rpc_modules(move |ctx| {
                    if !args.enable_ext {
                        return Ok(())
                    }

                    // here we get the configured provider.
                    let provider = ctx.provider().clone();

                    let ext = OpDepositCountExt::new(provider);

                    // now we merge our extension namespace into all configured transports
                    ctx.modules.merge_configured(ext)?;

                    println!("txpool extension enabled");

                    Ok(())
                })
                .launch()   
                .await?;

            handle.wait_for_node_exit().await
        })
        .unwrap();
}

/// Our custom cli args extension that adds one flag to reth default CLI.
#[derive(Debug, Clone, Copy, Default, clap::Args)]
struct RethCliOpDepositCountExt {
    #[arg(long)]
    pub enable_ext: bool,
}

/// trait interface for a custom rpc namespace: `opdepositcount`
///
/// This defines an additional namespace where all methods are configured as trait functions.
#[rpc[server, namespace="onDepositCount"]]
pub trait OpDepositCountExtApi {
    #[method(name = "opdepositCount")]
    fn op_deposit_count(&self) -> RpcResult<usize>;
}

pub struct OpDepositCountExt<Provider> {
    provider: Provider,
}

impl <P> OpDepositCountExt<P>
where
    P:EthFilterApiClient + Clone + Send + Sync + 'static
{
    pub fn new(provider : P ) -> OpDepositCountExt<P>{
        Self{provider}
    }
}

#[async_trait]
impl<P> OpDepositCountExtApiServer for OpDepositCountExt<P>
where
    P: EthFilterApiClient + Clone + Send + Sync+ 'static,
{
    async fn op_deposit_count(&self) -> RpcResult<usize> {
        let filter = Filter::new().select(0u64..).event("ETHBridgeFinalized(address,address,uint256)");
        let filter_res = self.provider.new_filter(filter).await;

       match filter_res{
        Ok(id) =>{
            let data = self.provider.filter_logs(id).await;
            let mut deposit_count = 0;
            match data{
                Ok(logs) =>{
                    for log in logs{
                       let val = log.log_decode::<L1StandardBridge::ETHBridgeFinalized>().ok();

                       if let Some(finalized_data) = val{
                        // let's  see what to do with this data later.
                            let da = finalized_data.data();
                            deposit_count += 1;
                       }
                    }
                    Ok(deposit_count)
                },
                Err(_) =>{
                   Ok(0)
                }
            }
        },
            Err(_)=>{
               Ok(0)
        }

       }
    }
}
