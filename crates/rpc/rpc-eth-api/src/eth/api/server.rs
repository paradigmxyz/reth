//! Implementation of the [`jsonrpsee`] generated [`reth_rpc_eth_api::EthApiServer`] trait
//! Handles RPC requests for the `eth_` namespace.

use jsonrpsee::core::RpcResult as Result;
use reth_evm::ConfigureEvm;
use reth_network_api::NetworkInfo;
use reth_primitives::{Address, BlockId, BlockNumberOrTag, Bytes, B256, B64, U256, U64};
use reth_provider::{
    BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, HeaderProvider, StateProviderFactory,
};
use reth_rpc_types::{
    serde_helpers::JsonStorageKey, state::StateOverride, AccessListWithGasUsed,
    AnyTransactionReceipt, BlockOverrides, Bundle, EIP1186AccountProofResponse, EthCallResponse,
    FeeHistory, Header, Index, RichBlock, StateContext, SyncStatus, TransactionRequest, Work,
};
use reth_transaction_pool::TransactionPool;
use serde_json::Value;
use tracing::trace;

use crate::{
    eth::{
        api::{EthApiSpec, EthBlocks, EthCall, EthFees, EthState, EthTransactions},
        error::EthApiError,
        revm_utils::EvmOverrides,
        EthApi,
    },
    result::{internal_rpc_err, ToRpcResult},
    EthApiServer,
};
