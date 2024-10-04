use alloy_primitives::{Bytes, TxKind, U256};
use alloy_rpc_types_eth::transaction::TransactionRequest;
use reth_chainspec::EthereumHardforks;
use reth_evm::ConfigureEvm;
use reth_node_api::{FullNodeComponents, NodeTypes};
use reth_primitives::{
    revm_primitives::{BlockEnv, OptimismFields, TxEnv},
    Header,
};
use reth_rpc_eth_api::{
    helpers::{Call, EthCall, LoadState, SpawnBlocking},
    FromEthApiError, IntoEthApiError,
};
use reth_rpc_eth_types::{revm_utils::CallFees, RpcInvalidTransactionError};

use crate::{OpEthApi, OpEthApiError};

impl<N> EthCall for OpEthApi<N>
where
    Self: Call,
    N: FullNodeComponents<Types: NodeTypes<ChainSpec: EthereumHardforks>>,
{
}

impl<N> Call for OpEthApi<N>
where
    Self: LoadState + SpawnBlocking,
    Self::Error: From<OpEthApiError>,
    N: FullNodeComponents,
{
    #[inline]
    fn call_gas_limit(&self) -> u64 {
        self.inner.gas_cap()
    }

    #[inline]
    fn max_simulate_blocks(&self) -> u64 {
        self.inner.max_simulate_blocks()
    }

    #[inline]
    fn evm_config(&self) -> &impl ConfigureEvm<Header = Header> {
        self.inner.evm_config()
    }

    fn create_txn_env(
        &self,
        block_env: &BlockEnv,
        request: TransactionRequest,
    ) -> Result<TxEnv, Self::Error> {
        // Ensure that if versioned hashes are set, they're not empty
        if request.blob_versioned_hashes.as_ref().map_or(false, |hashes| hashes.is_empty()) {
            return Err(RpcInvalidTransactionError::BlobTransactionMissingBlobHashes.into_eth_err())
        }

        let TransactionRequest {
            from,
            to,
            gas_price,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            gas,
            value,
            input,
            nonce,
            access_list,
            chain_id,
            blob_versioned_hashes,
            max_fee_per_blob_gas,
            authorization_list,
            ..
        } = request;

        let CallFees { max_priority_fee_per_gas, gas_price, max_fee_per_blob_gas } =
            CallFees::ensure_fees(
                gas_price.map(U256::from),
                max_fee_per_gas.map(U256::from),
                max_priority_fee_per_gas.map(U256::from),
                block_env.basefee,
                blob_versioned_hashes.as_deref(),
                max_fee_per_blob_gas.map(U256::from),
                block_env.get_blob_gasprice().map(U256::from),
            )?;

        let gas_limit = gas.unwrap_or_else(|| block_env.gas_limit.min(U256::from(u64::MAX)).to());

        #[allow(clippy::needless_update)]
        let env = TxEnv {
            gas_limit,
            nonce,
            caller: from.unwrap_or_default(),
            gas_price,
            gas_priority_fee: max_priority_fee_per_gas,
            transact_to: to.unwrap_or(TxKind::Create),
            value: value.unwrap_or_default(),
            data: input
                .try_into_unique_input()
                .map_err(Self::Error::from_eth_err)?
                .unwrap_or_default(),
            chain_id,
            access_list: access_list.unwrap_or_default().into(),
            // EIP-4844 fields
            blob_hashes: blob_versioned_hashes.unwrap_or_default(),
            max_fee_per_blob_gas,
            authorization_list: authorization_list.map(Into::into),
            optimism: OptimismFields { enveloped_tx: Some(Bytes::new()), ..Default::default() },
        };

        Ok(env)
    }
}
