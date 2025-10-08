use serde::{Deserialize, Serialize};
use alloy_consensus::error::ValueError;
use alloy_rpc_types_eth::request::TransactionRequest as EthTransactionRequest;
use revm_context::{cfg::CfgEnv, block::BlockEnv};
use revm_context::tx::TxEnv;
use reth_rpc_convert::transaction::{TryIntoSimTx, TryIntoTxEnv, EthTxEnvError};
use reth_arbitrum_primitives::ArbTransactionSigned;
use reth_arbitrum_evm::ArbTransaction;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArbTransactionRequest(pub EthTransactionRequest);

impl AsRef<EthTransactionRequest> for ArbTransactionRequest {
    fn as_ref(&self) -> &EthTransactionRequest {
        &self.0
    }
}

impl AsMut<EthTransactionRequest> for ArbTransactionRequest {
    fn as_mut(&mut self) -> &mut EthTransactionRequest {
        &mut self.0
    }
}

impl TryIntoSimTx<ArbTransactionSigned> for ArbTransactionRequest {
    fn try_into_sim_tx(self) -> Result<ArbTransactionSigned, ValueError<Self>> {
        Err(ValueError::new(self, "simulate_v1 not yet supported for Arbitrum"))
    }
}

impl TryIntoTxEnv<ArbTransaction<TxEnv>> for ArbTransactionRequest {
    type Err = EthTxEnvError;

    fn try_into_tx_env<Spec>(
        self,
        cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<ArbTransaction<TxEnv>, Self::Err> {
        let base = self.as_ref().clone().try_into_tx_env(cfg_env, block_env)?;
        Ok(ArbTransaction { 0: base })
    }
}

use crate::eth::ArbEthApi;
use alloy_primitives::{Bytes, B256};
use reth_rpc_eth_api::{
    helpers::{spec::SignersForRpc, EthTransactions, LoadTransaction},
    FromEvmError, FromEthApiError, RpcConvert, RpcNodeCore,
};
use crate::error::ArbEthApiError;
use reth_transaction_pool::{AddedTransactionOutcome, PoolTransaction, TransactionOrigin, TransactionPool};
use reth_rpc_eth_types::utils::recover_raw_transaction;

impl<N, Rpc> EthTransactions for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    ArbEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = ArbEthApiError>,
{
    fn signers(&self) -> &SignersForRpc<Self::Provider, Self::NetworkTypes> {
        self.eth_api().signers()
    }

    fn send_raw_transaction_sync_timeout(&self) -> std::time::Duration {
        self.eth_api().send_raw_transaction_sync_timeout()
    }

    async fn send_raw_transaction(&self, tx: Bytes) -> Result<B256, Self::Error> {
        let recovered = recover_raw_transaction(&tx)?;

        self.eth_api().broadcast_raw_transaction(tx.clone());

        let pool_tx = <Self::Pool as TransactionPool>::Transaction::from_pooled(recovered);
        let AddedTransactionOutcome { hash, .. } = self
            .pool()
            .add_transaction(TransactionOrigin::Local, pool_tx)
            .await
            .map_err(Self::Error::from_eth_err)?;

        Ok(hash)
    }
}

impl<N, Rpc> LoadTransaction for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = ArbEthApiError>,
{
}
