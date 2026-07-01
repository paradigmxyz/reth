//! L1 `eth` API types.

use alloy_network::Ethereum;
use reth_evm_ethereum::EthEvmConfig;
use reth_rpc_convert::RpcConverter;
use reth_rpc_eth_types::receipt::EthReceiptConverter;

/// An [`RpcConverter`] with its generics set to Ethereum specific.
pub type EthRpcConverter<ChainSpec> =
    RpcConverter<Ethereum, EthEvmConfig, EthReceiptConverter<ChainSpec>>;
