//! Contains RPC handler implementations specific to sign endpoints

use crate::{
    eth::error::{EthApiError, EthResult},
    EthApi,
};
use reth_primitives::{Address, Bytes, Signature};
use reth_provider::{BlockProvider, EvmEnvProvider, StateProviderFactory};
use reth_transaction_pool::TransactionPool;

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Pool: TransactionPool + 'static,
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Network: 'static,
{
    pub(crate) async fn sign(&self, account: Address, message: Bytes) -> EthResult<Bytes> {
        let signer = self
            .inner
            .signers
            .iter()
            .find(|signer| signer.is_signer_for(&account))
            .ok_or(EthApiError::NoSigner)?;
        let signature = signer.sign(account, &message).await.map_err(|_err| EthApiError::NoSigner)?;
        let mut buff = vec![];
        Signature::encode(&signature, &mut buff);
        Ok(buff.into())
    }
}
