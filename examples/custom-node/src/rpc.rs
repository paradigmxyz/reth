use crate::{
    evm::CustomTxEnv,
    primitives::{CustomHeader, CustomTransaction},
};
use alloy_consensus::error::ValueError;
use alloy_network::TxSigner;
use op_alloy_consensus::OpTxEnvelope;
use op_alloy_rpc_types::{OpTransactionReceipt, OpTransactionRequest};
use reth_op::rpc::RpcTypes;
use reth_rpc_api::eth::{
    transaction::TryIntoTxEnv, EthTxEnvError, SignTxRequestError, SignableTxRequest, TryIntoSimTx,
};
use revm::context::{BlockEnv, CfgEnv};

#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct CustomRpcTypes;

impl RpcTypes for CustomRpcTypes {
    type Header = alloy_rpc_types_eth::Header<CustomHeader>;
    type Receipt = OpTransactionReceipt;
    type TransactionRequest = OpTransactionRequest;
    type TransactionResponse = op_alloy_rpc_types::Transaction<CustomTransaction>;
}

impl TryIntoSimTx<CustomTransaction> for OpTransactionRequest {
    fn try_into_sim_tx(self) -> Result<CustomTransaction, ValueError<Self>> {
        Ok(CustomTransaction::Op(self.try_into_sim_tx()?))
    }
}

impl TryIntoTxEnv<CustomTxEnv> for OpTransactionRequest {
    type Err = EthTxEnvError;

    fn try_into_tx_env<Spec>(
        self,
        cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<CustomTxEnv, Self::Err> {
        Ok(CustomTxEnv::Op(self.try_into_tx_env(cfg_env, block_env)?))
    }
}

impl SignableTxRequest<CustomTransaction> for OpTransactionRequest {
    async fn try_build_and_sign(
        self,
        signer: impl TxSigner<alloy_primitives::Signature> + Send,
    ) -> Result<CustomTransaction, SignTxRequestError> {
        Ok(CustomTransaction::Op(
            SignableTxRequest::<OpTxEnvelope>::try_build_and_sign(self, signer).await?,
        ))
    }
}
