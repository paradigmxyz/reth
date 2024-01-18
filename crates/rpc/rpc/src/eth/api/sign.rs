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
use std::ops::Deref;

impl<Provider, Pool, Network> EthApi<Provider, Pool, Network> {
    pub(crate) async fn sign(&self, account: Address, message: Bytes) -> EthResult<Bytes> {
        let signer = self.find_signer(&account)?;
        let signature = signer.sign(account, &message).await?;
        Ok(signature.to_hex_bytes())
    }

    pub(crate) async fn sign_typed_data(&self, data: Value, account: Address) -> EthResult<Bytes> {
        let signer = self.find_signer(&account)?;
        let data =
            serde_json::from_value::<TypedData>(data).map_err(|_| SignError::InvalidTypedData)?;
        let signature = signer.sign_typed_data(account, &data)?;
        Ok(signature.to_hex_bytes())
    }

    pub(crate) fn find_signer(
        &self,
        account: &Address,
    ) -> Result<&(dyn EthSigner + 'static), SignError> {
        self.inner
            .signers
            .iter()
            .find(|signer| signer.is_signer_for(account))
            .map(|signer| signer.deref())
            .ok_or(SignError::NoAccount)
    }
}
