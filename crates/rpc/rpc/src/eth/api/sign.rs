//! Contains RPC handler implementations specific to sign endpoints

use crate::{
    eth::{
        error::{EthResult, SignError},
        signer::EthSigner,
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
    ) -> Result<Box<(dyn EthSigner + 'static)>, SignError> {
        self.inner
            .signers
            .read()
            .await
            .iter()
            .find(|signer| signer.is_signer_for(account))
            .map(|signer| dyn_clone::clone_box(&**signer))
            .ok_or(SignError::NoAccount)
    }

    pub(crate) async fn add_signer<S: EthSigner + 'static>(&self, s: S) -> EthResult<()> {
        self.inner.signers.write().await.push(std::boxed::Box::new(s));
        Ok(())
    }
}
