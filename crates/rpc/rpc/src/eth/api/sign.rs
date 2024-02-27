//! Contains RPC handler implementations specific to sign endpoints

use crate::{
    eth::{
        error::{EthResult, SignError},
        signer::{DevSigner, EthSigner},
    },
    EthApi,
};
use alloy_dyn_abi::TypedData;
use reth_primitives::{Address, Bytes};
use serde_json::Value;

impl<Provider, Pool, Network, EvmConfig> EthApi<Provider, Pool, Network, EvmConfig> {
    pub(crate) async fn sign(&self, account: Address, message: Bytes) -> EthResult<Bytes> {
        Ok(self.find_signer(&account).await?.sign(account, &message).await?.to_hex_bytes())
    }

    pub(crate) async fn sign_typed_data(&self, data: Value, account: Address) -> EthResult<Bytes> {
        Ok(self
            .find_signer(&account)
            .await?
            .sign_typed_data(
                account,
                &serde_json::from_value::<TypedData>(data)
                    .map_err(|_| SignError::InvalidTypedData)?,
            )?
            .to_hex_bytes())
    }

    pub(crate) async fn find_signer(
        &self,
        account: &Address,
    ) -> Result<Box<(dyn EthSigner + Send + Sync + 'static)>, SignError> {
        self.inner
            .signers
            .lock()
            .await
            .iter()
            .find(|signer| signer.is_signer_for(account))
            .map(|signer| signer.clone_box())
            .ok_or(SignError::NoAccount)
    }

    pub(crate) async fn add_signer(&self) -> EthResult<()> {
        let dev_signer = DevSigner::new();
        self.inner.signers.lock().await.push(std::boxed::Box::new(dev_signer));
        Ok(())
    }
}
