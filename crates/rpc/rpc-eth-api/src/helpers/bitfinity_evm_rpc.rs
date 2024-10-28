//! Implements Bifitnity EVM RPC methods.

use std::sync::Arc;

use ethereum_json_rpc_client::{reqwest::ReqwestClient, EthJsonRpcClient};
use ethereum_json_rpc_client::{Block, CertifiedResult, H256};
use futures::Future;
use jsonrpsee::core::RpcResult;
use reth_chainspec::ChainSpec;
use reth_rpc_server_types::result::internal_rpc_err;
use revm_primitives::{Address, Bytes, B256, U256};

/// Proxy to the Bitfinity EVM RPC.
pub trait BitfinityEvmRpc {
    /// Returns the ChainSpec.
    fn chain_spec(&self) -> Arc<ChainSpec>;

    /// Forwards `eth_gasPrice` calls to the Bitfinity EVM.
    fn gas_price(&self) -> impl Future<Output = RpcResult<U256>> + Send {
        let chain_spec = self.chain_spec();
        async move {
            let (rpc_url, client) = get_client(&chain_spec)?;

            let gas_price = client.gas_price().await.map_err(|e| {
                internal_rpc_err(format!(
                    "failed to forward eth_gasPrice request to {}: {}",
                    rpc_url, e
                ))
            })?;

            Ok(U256::from(gas_price.as_u128()))
        }
    }

    /// Forwards `eth_maxPriorityFeePerGas` calls to the Bitfinity EVM
    fn max_priority_fee_per_gas(&self) -> impl Future<Output = RpcResult<U256>> + Send {
        let chain_spec = self.chain_spec();
        async move {
            let (rpc_url, client) = get_client(&chain_spec)?;

            let priority_fee = client.max_priority_fee_per_gas().await.map_err(|e| {
                internal_rpc_err(format!(
                    "failed to forward eth_maxPriorityFeePerGas request to {}: {}",
                    rpc_url, e
                ))
            })?;

            Ok(U256::from(priority_fee.as_u128()))
        }
    }

    /// Forwards `eth_sendRawTransaction` calls to the Bitfinity EVM
    fn send_raw_transaction(&self, tx: Bytes) -> impl Future<Output = RpcResult<B256>> + Send {
        let chain_spec = self.chain_spec();
        async move {
            let (rpc_url, client) = get_client(&chain_spec)?;

            let tx_hash = client.send_raw_transaction_bytes(&tx).await.map_err(|e| {
                internal_rpc_err(format!(
                    "failed to forward eth_sendRawTransaction request to {}: {}",
                    rpc_url, e
                ))
            })?;

            Ok(tx_hash.0.into())
        }
    }

    /// Forwards `ic_getGenesisBalances` calls to the Bitfinity EVM
    fn get_genesis_balances(&self) -> impl Future<Output = RpcResult<Vec<(Address, U256)>>> + Send {
        let chain_spec = self.chain_spec();
        async move {
            let (rpc_url, client) = get_client(&chain_spec)?;

            let balances = client.get_genesis_balances().await.map_err(|e| {
                internal_rpc_err(format!(
                    "failed to forward ic_getGenesisBalances request to {}: {}",
                    rpc_url, e
                ))
            })?;

            Ok(balances
                .into_iter()
                .map(|(address, balance)| (address.0.into(), U256::from(balance.as_u128())))
                .collect())
        }
    }

    /// Forwards `ic_getLastCertifiedBlock` calls to the Bitfinity EVM
    fn get_last_certified_block(
        &self,
    ) -> impl Future<Output = RpcResult<CertifiedResult<Block<H256>>>> + Send {
        let chain_spec = self.chain_spec();
        async move {
            let (rpc_url, client) = get_client(&chain_spec)?;

            let certified_block  = client.get_last_certified_block().await.map_err(|e| {
                internal_rpc_err(format!(
                    "failed to forward get_last_certified_block request to {}: {}",
                    rpc_url, e
                ))
            })?;

            Ok(certified_block)
        }
    }
}

/// Returns a client for the Bitfinity EVM RPC.
fn get_client(chain_spec: &ChainSpec) -> RpcResult<(&String, EthJsonRpcClient<ReqwestClient>)> {
    let Some(rpc_url) = &chain_spec.bitfinity_evm_url else {
        return Err(internal_rpc_err("bitfinity_evm_url not found in chain spec"));
    };

    let client = ethereum_json_rpc_client::EthJsonRpcClient::new(
        ethereum_json_rpc_client::reqwest::ReqwestClient::new(rpc_url.to_string()),
    );

    Ok((rpc_url, client))
}
